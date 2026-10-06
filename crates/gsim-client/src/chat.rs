//! The chat box, modelled on Minecraft's: recent lines that fade, scrollback while open,
//! an input line with history, a suggestion list above the word being typed (arrows to move,
//! Tab to accept and cycle), a grey usage hint or red error when there is nothing to
//! suggest, and clickable mentions of players and bodies.

use crate::style;
use egui_macroquad::egui;
use gsim_client_core::complete::{self, Analysis, Context, SpanKind};
use gsim_client_core::ChatEntry;
use gsim_proto::{ChatKind, PlayerId, MAX_CHAT_CHARS};
use std::collections::VecDeque;

/// Seconds a line stays up while the box is closed, and how long its fade-out takes.
const LINGER: f64 = 10.0;
const FADE: f64 = 2.0;
const LINES_CLOSED: usize = 6;
const LINES_OPEN: usize = 14;
const WIDTH: f32 = 380.0;
/// Rows of the suggestion list shown at once (as in Minecraft's chat).
const LIST_ROWS: usize = 10;
/// Chat lines scrolled per wheel notch (one with Shift held).
const WHEEL_LINES: usize = 3;

// Minecraft's palette for the pieces of a command.
const C_LITERAL: egui::Color32 = egui::Color32::from_rgb(170, 170, 170);
const C_ERROR: egui::Color32 = egui::Color32::from_rgb(255, 85, 85);
const C_SELECTED: egui::Color32 = egui::Color32::from_rgb(255, 255, 0);
const C_GHOST: egui::Color32 = egui::Color32::from_rgb(128, 128, 128);
const C_ARGS: [egui::Color32; 5] = [
    egui::Color32::from_rgb(85, 255, 255),
    egui::Color32::from_rgb(255, 255, 85),
    egui::Color32::from_rgb(85, 255, 85),
    egui::Color32::from_rgb(255, 85, 255),
    egui::Color32::from_rgb(255, 170, 0),
];
const C_POPUP: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 0, 0, 208);

/// Something a word in chat can refer to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mention {
    Body(u32),
    Player(PlayerId),
}

#[derive(Default)]
pub struct Outcome {
    /// A line to send to the server.
    pub send: Option<String>,
    pub clicked: Option<Mention>,
}

/// The open suggestion list.
struct List {
    /// The input the suggestions were computed for, and where the completed word starts.
    base: String,
    start: usize,
    items: Vec<String>,
    selected: usize,
    /// First visible row.
    offset: usize,
    /// Tab moves to the next entry before accepting (after the first accept).
    tab_cycles: bool,
    /// The input as left by the last accept: while it is unchanged the list stays.
    applied: Option<String>,
}

impl List {
    fn cycle(&mut self, by: isize) {
        let n = self.items.len() as isize;
        self.selected = (self.selected as isize + by).rem_euclid(n) as usize;
        self.offset = self.offset.min(self.selected).max((self.selected + 1).saturating_sub(LIST_ROWS));
    }
}

#[derive(Default)]
pub struct ChatBox {
    pub open: bool,
    input: String,
    history: Vec<String>,
    /// Position in `history` while browsing with the arrow keys.
    recall: Option<usize>,
    /// Frames left in which to take focus and put the cursor at the end.
    settle: u8,
    opened_with: String,
    /// Lines scrolled up from the newest.
    scroll: usize,
    list: Option<List>,
    /// What the analysis below was computed for.
    analysed: String,
    analysis: Analysis,
    /// Escape hid the suggestions for exactly this input.
    hidden_for: Option<String>,
}

impl ChatBox {
    /// Open the input line, optionally with something already typed (`/` for a command).
    pub fn open_with(&mut self, text: &str) {
        self.open = true;
        self.input = text.to_string();
        self.opened_with = text.to_string();
        self.settle = 5;
        self.recall = None;
        self.scroll = 0;
        self.list = None;
        self.hidden_for = None;
        // Force a fresh analysis even if the text matches the last one.
        self.analysed = "\u{0}".to_string();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.input.clear();
        self.list = None;
        self.scroll = 0;
    }

    /// Escape: hide the suggestion list if it is showing, otherwise close the chat.
    pub fn escape(&mut self) {
        if self.list.take().is_some() {
            self.hidden_for = Some(self.input.clone());
        } else {
            self.close();
        }
    }

    /// Draw the box in the bottom-left corner. `mentions` are the names worth highlighting.
    pub fn show(&mut self, ctx: &egui::Context, log: &VecDeque<ChatEntry>, now: f64, complete_ctx: &Context, mentions: &[(String, Mention)]) -> Outcome {
        let mut out = Outcome::default();
        let visible: Vec<&ChatEntry> = if self.open {
            self.scroll = self.scroll.min(log.len().saturating_sub(LINES_OPEN));
            log.iter().rev().skip(self.scroll).take(LINES_OPEN).collect()
        } else {
            log.iter().rev().take_while(|e| now - e.at < LINGER).take(LINES_CLOSED).collect()
        };
        if visible.is_empty() && !self.open {
            return out;
        }
        egui::Area::new(egui::Id::new("chat")).anchor(egui::Align2::LEFT_BOTTOM, [10.0, -10.0]).show(ctx, |ui| {
            let frame = if self.open { style::panel() } else { egui::Frame::NONE.inner_margin(10.0) };
            frame.show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 2.0;
                for entry in visible.iter().rev() {
                    let age = now - entry.at;
                    let opacity = if self.open { 1.0 } else { ((LINGER - age) / FADE).clamp(0.0, 1.0) as f32 };
                    if let Some(hit) = line(ui, entry, opacity, mentions) {
                        out.clicked = Some(hit);
                    }
                }
                if self.open {
                    if self.scroll > 0 {
                        ui.label(egui::RichText::new(format!("- {} newer -", self.scroll)).small().color(style::c32(style::DIM)));
                    }
                    ui.add_space(4.0);
                    out.send = self.input_row(ui, complete_ctx);
                }
            });
        });
        out
    }

    /// Recompute suggestions, hint and colours if the input changed since the last look.
    fn refresh(&mut self, ctx: &Context) {
        if self.analysed == self.input {
            return;
        }
        self.analysed = self.input.clone();
        self.analysis = complete::analyze(&self.input, ctx, false);
        // An accepted suggestion keeps its list, so Tab can walk on through it.
        if self.list.as_ref().is_some_and(|l| l.applied.as_deref() == Some(self.input.as_str())) {
            return;
        }
        self.hidden_for = None;
        self.list = self.new_list(&self.analysis);
    }

    fn new_list(&self, analysis: &Analysis) -> Option<List> {
        (!analysis.suggestions.is_empty()).then(|| List {
            base: self.input.clone(),
            start: analysis.start,
            items: analysis.suggestions.clone(),
            selected: 0,
            offset: 0,
            tab_cycles: false,
            applied: None,
        })
    }

    /// Put the selected suggestion into the input (replacing the word being typed).
    fn accept(&mut self) {
        let Some(list) = self.list.as_mut() else { return };
        self.input = complete::apply(&list.base, list.start, &list.items[list.selected]);
        list.applied = Some(self.input.clone());
        list.tab_cycles = true;
    }

    fn input_row(&mut self, ui: &mut egui::Ui, complete_ctx: &Context) -> Option<String> {
        // A key that opened the chat may also arrive as typed text a frame later.
        if self.settle > 0 && self.input.len() > self.opened_with.len() && self.input.chars().all(|c| "tT/".contains(c)) {
            self.input = self.opened_with.clone();
        }
        self.input.retain(|c| c != '\t');
        self.refresh(complete_ctx);

        // Take the navigation keys before the text field sees them: it would move the cursor
        // on the arrows and insert a tab.
        let (tab, up, down, page_up, page_down, shift) = ui.input_mut(|i| {
            let shift = i.modifiers.shift;
            let mut take = |key| i.consume_key(egui::Modifiers::NONE, key) | i.consume_key(egui::Modifiers::SHIFT, key);
            (take(egui::Key::Tab), take(egui::Key::ArrowUp), take(egui::Key::ArrowDown), take(egui::Key::PageUp), take(egui::Key::PageDown), shift)
        });
        let id = egui::Id::new("chat-input");
        let font = egui::TextStyle::Body.resolve(ui.style());
        let plain = style::c32(style::TEXT);
        let spans = self.analysis.spans.clone();
        let analysed = self.analysed.clone();
        let layout_font = font.clone();
        let mut layouter = move |ui: &egui::Ui, text: &str, _wrap: f32| {
            let mut job = egui::text::LayoutJob::default();
            let piece = |colour| egui::TextFormat { font_id: layout_font.clone(), color: colour, ..Default::default() };
            // Colours belong to the analysed text; anything typed since is drawn plain.
            let mut at = 0;
            if text == analysed {
                for (start, end, kind) in &spans {
                    let (start, end) = ((*start).min(text.len()), (*end).min(text.len()));
                    if start < at || end <= start || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                        continue;
                    }
                    job.append(&text[at..start], 0.0, piece(plain));
                    let colour = match kind {
                        SpanKind::Literal => C_LITERAL,
                        SpanKind::Arg(i) => C_ARGS[i % C_ARGS.len()],
                        SpanKind::Error => C_ERROR,
                    };
                    job.append(&text[start..end], 0.0, piece(colour));
                    at = end;
                }
            }
            job.append(&text[at..], 0.0, piece(plain));
            ui.fonts(|f| f.layout_job(job))
        };
        let edit = egui::TextEdit::singleline(&mut self.input)
            .id(id)
            .desired_width(f32::INFINITY)
            .char_limit(MAX_CHAT_CHARS)
            // Keep Tab for suggestions instead of moving focus.
            .lock_focus(true)
            .layouter(&mut layouter)
            .show(ui);
        let response = edit.response;
        let text_left = edit.galley_pos.x;
        let text_end = edit.galley_pos.x + edit.galley.size().x;
        let text_top = edit.galley_pos.y;
        let mut cursor_to_end = false;
        if self.settle > 0 {
            self.settle -= 1;
            response.request_focus();
            cursor_to_end = true;
        }

        // --- keys -------------------------------------------------------------------------
        let (enter, wheel) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.raw_scroll_delta.y));
        if let Some(list) = self.list.as_mut() {
            // Arrows move through the list; Tab accepts, and keeps walking on repeats.
            if up || down {
                list.cycle(if up { -1 } else { 1 });
                list.tab_cycles = false;
            } else if tab {
                if list.tab_cycles {
                    list.cycle(if shift { -1 } else { 1 });
                }
                self.accept();
                cursor_to_end = true;
            }
        } else if tab {
            // Nothing showing: ask for suggestions (this is how names complete in plain chat).
            let forced = complete::analyze(&self.input, complete_ctx, true);
            self.list = self.new_list(&forced);
            self.hidden_for = None;
        } else if up || down {
            cursor_to_end = self.recall(up);
        }
        if page_up || page_down {
            self.scroll = if page_up { self.scroll + LINES_OPEN - 1 } else { self.scroll.saturating_sub(LINES_OPEN - 1) };
        }

        // --- popups above the input ---------------------------------------------------------
        let width_of = |text: &str| ui.fonts(|f| f.layout_no_wrap(text.to_string(), font.clone(), plain).size().x);
        let mut over_list = false;
        let mut clicked_row = None;
        if let Some(list) = self.list.as_mut() {
            let x = text_left + width_of(&list.base[..list.start.min(list.base.len())]);
            let rows = list.items.len().min(LIST_ROWS);
            let row_h = font.size + 2.0;
            let w = list.items.iter().map(|s| width_of(s)).fold(0.0, f32::max) + 8.0;
            let rect = egui::Rect::from_min_size(egui::pos2(x - 2.0, text_top - 6.0 - rows as f32 * row_h), egui::vec2(w, rows as f32 * row_h));
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("chat-suggestions")));
            painter.rect_filled(rect, 0.0, C_POPUP);
            let pointer = ui.input(|i| i.pointer.hover_pos());
            over_list = pointer.is_some_and(|p| rect.contains(p));
            if over_list && wheel != 0.0 {
                let max = list.items.len().saturating_sub(LIST_ROWS);
                list.offset = if wheel > 0.0 { list.offset.saturating_sub(1) } else { (list.offset + 1).min(max) };
            }
            for row in 0..rows {
                let i = list.offset + row;
                let top = rect.top() + row as f32 * row_h;
                let row_rect = egui::Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(w, row_h));
                if pointer.is_some_and(|p| row_rect.contains(p)) {
                    // Hovering selects, clicking accepts.
                    list.selected = i;
                    if ui.input(|inp| inp.pointer.primary_clicked()) {
                        clicked_row = Some(i);
                    }
                }
                let colour = if i == list.selected { C_SELECTED } else { C_LITERAL };
                painter.text(egui::pos2(rect.left() + 3.0, top + 1.0), egui::Align2::LEFT_TOP, &list.items[i], font.clone(), colour);
            }
            // Dotted edges where the list continues, as in Minecraft.
            let dots = |y: f32| {
                let mut dx = rect.left() + 1.0;
                while dx < rect.right() {
                    painter.rect_filled(egui::Rect::from_min_size(egui::pos2(dx, y), egui::vec2(1.0, 1.0)), 0.0, egui::Color32::WHITE);
                    dx += 2.0;
                }
            };
            if list.offset > 0 {
                dots(rect.top());
            }
            if list.offset + rows < list.items.len() {
                dots(rect.bottom() - 1.0);
            }
            // Grey preview of what accepting the selected entry would add.
            if list.applied.is_none() {
                if let Some(rest) = complete::suffix(&list.base, list.start, &list.items[list.selected]).filter(|_| list.base == self.input) {
                    ui.painter().text(egui::pos2(text_end, text_top), egui::Align2::LEFT_TOP, rest, font.clone(), C_GHOST);
                }
            }
        } else if self.hidden_for.as_deref() != Some(self.input.as_str()) {
            // No list: the red error, or the grey hint of what is still expected.
            let (text, colour, x) = match (&self.analysis.error, &self.analysis.usage) {
                (Some(error), _) => (error.as_str(), C_ERROR, text_left),
                (None, Some(usage)) => (usage.as_str(), C_LITERAL, text_left + width_of(&self.input[..self.analysis.start.min(self.input.len())])),
                _ => ("", C_LITERAL, 0.0),
            };
            if !text.is_empty() {
                let row_h = font.size + 2.0;
                let rect = egui::Rect::from_min_size(egui::pos2(x - 2.0, text_top - 6.0 - row_h), egui::vec2(width_of(text) + 8.0, row_h));
                let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("chat-suggestions")));
                painter.rect_filled(rect, 0.0, C_POPUP);
                painter.text(egui::pos2(rect.left() + 3.0, rect.top() + 1.0), egui::Align2::LEFT_TOP, text, font.clone(), colour);
            }
        }
        if let Some(i) = clicked_row {
            if let Some(list) = self.list.as_mut() {
                list.selected = i;
            }
            self.accept();
            cursor_to_end = true;
            response.request_focus();
        }
        // The wheel scrolls the chat history unless it is over the suggestion list.
        if wheel != 0.0 && !over_list {
            let lines = if shift { 1 } else { WHEEL_LINES };
            self.scroll = if wheel > 0.0 { self.scroll + lines } else { self.scroll.saturating_sub(lines) };
        }

        if cursor_to_end {
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                let end = egui::text::CCursor::new(self.input.chars().count());
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                state.store(ui.ctx(), id);
            }
        }
        if enter {
            let text = self.input.trim().to_string();
            self.close();
            if !text.is_empty() {
                self.history.retain(|h| *h != text);
                self.history.push(text.clone());
                return Some(text);
            }
        } else if !response.has_focus() {
            // Clicked elsewhere: keep typing where we were.
            response.request_focus();
        }
        None
    }

    /// Up / Down with no list showing: walk through what was sent before.
    fn recall(&mut self, older: bool) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let last = self.history.len() - 1;
        self.recall = match (self.recall, older) {
            (None, true) => Some(last),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i < last => Some(i + 1),
            _ => None,
        };
        self.input = self.recall.map_or(String::new(), |i| self.history[i].clone());
        true
    }
}

/// Where each mention sits in `text`: (start, end, target), longest names first, no overlaps.
fn find_mentions(text: &str, mentions: &[(String, Mention)]) -> Vec<(usize, usize, Mention)> {
    let lower = text.to_lowercase();
    let boundary = |i: usize| i == 0 || i >= lower.len() || !lower.as_bytes()[i].is_ascii_alphanumeric() || !lower.as_bytes()[i - 1].is_ascii_alphanumeric();
    let mut names: Vec<&(String, Mention)> = mentions.iter().filter(|m| m.0.len() >= 2).collect();
    names.sort_by_key(|m| std::cmp::Reverse(m.0.len()));
    let mut found: Vec<(usize, usize, Mention)> = Vec::new();
    for (name, target) in names {
        let needle = name.to_lowercase();
        let mut from = 0;
        while let Some(pos) = lower[from..].find(&needle) {
            let (start, end) = (from + pos, from + pos + needle.len());
            from = end;
            let whole_word = boundary(start) && boundary(end);
            if whole_word && lower.len() == text.len() && !found.iter().any(|f| start < f.1 && f.0 < end) {
                found.push((start, end, *target));
            }
        }
    }
    found.sort_by_key(|f| f.0);
    found
}

/// One chat line. Returns the mention that was clicked, if any.
fn line(ui: &mut egui::Ui, entry: &ChatEntry, opacity: f32, mentions: &[(String, Mention)]) -> Option<Mention> {
    let fade = |c| style::c32(style::alpha(c, opacity));
    let (prefix, prefix_colour, text_colour) = match (&entry.kind, &entry.from) {
        (ChatKind::Say, Some(from)) => (format!("{from}"), style::OTHER_SHIP, style::TEXT),
        (ChatKind::Private { outgoing: true }, Some(to)) => (format!("to {to}"), style::VIOLET, style::VIOLET),
        (ChatKind::Private { .. }, Some(from)) => (format!("from {from}"), style::VIOLET, style::VIOLET),
        (ChatKind::Error, _) => (String::new(), style::EMBER, style::EMBER),
        _ => (String::new(), style::DIM, style::DIM),
    };
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if !prefix.is_empty() {
            ui.label(egui::RichText::new(format!("{prefix}  ")).color(fade(prefix_colour)));
        }
        let mut at = 0;
        for (start, end, target) in find_mentions(&entry.text, mentions) {
            if start > at {
                ui.label(egui::RichText::new(&entry.text[at..start]).color(fade(text_colour)));
            }
            let colour = match target {
                Mention::Body(_) => style::GOLD,
                Mention::Player(_) => style::OTHER_SHIP,
            };
            let word = egui::Label::new(egui::RichText::new(&entry.text[start..end]).color(fade(colour)).underline()).sense(egui::Sense::click());
            if ui.add(word).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                clicked = Some(target);
            }
            at = end;
        }
        if at < entry.text.len() {
            ui.label(egui::RichText::new(&entry.text[at..]).color(fade(text_colour)));
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_are_whole_words_and_prefer_longer_names() {
        let names = vec![
            ("Sun".to_string(), Mention::Body(0)),
            ("B45".to_string(), Mention::Body(45)),
            ("B459".to_string(), Mention::Body(459)),
            ("ann".to_string(), Mention::Player(1)),
        ];
        let hits = |t: &str| find_mentions(t, &names).into_iter().map(|(s, e, m)| (t[s..e].to_string(), m)).collect::<Vec<_>>();
        assert_eq!(hits("meet at B459, ann!"), [("B459".to_string(), Mention::Body(459)), ("ann".to_string(), Mention::Player(1))]);
        assert_eq!(hits("the SUN is hot"), [("SUN".to_string(), Mention::Body(0))]);
        assert!(hits("sunday planning, banner").is_empty(), "no matches inside other words");
    }
}
