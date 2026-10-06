//! The chat box: recent lines that fade, an input line with history and tab completion,
//! and clickable mentions of players and bodies.

use crate::style;
use egui_macroquad::egui;
use gsim_client_core::complete::{self, Context};
use gsim_client_core::ChatEntry;
use gsim_proto::{ChatKind, PlayerId, MAX_CHAT_CHARS};
use std::collections::VecDeque;

/// Seconds a line stays up while the box is closed, and how long its fade-out takes.
const LINGER: f64 = 10.0;
const FADE: f64 = 2.0;
const LINES_CLOSED: usize = 6;
const LINES_OPEN: usize = 14;
const WIDTH: f32 = 380.0;

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

struct Completion {
    /// What was typed when Tab was first pressed.
    base: String,
    candidates: Vec<String>,
    index: usize,
    /// The input after the last completion, to tell whether the player typed since.
    applied: String,
}

#[derive(Default)]
pub struct ChatBox {
    pub open: bool,
    input: String,
    history: Vec<String>,
    /// Position in `history` while browsing with the arrow keys.
    recall: Option<usize>,
    completion: Option<Completion>,
    grab_focus: bool,
}

impl ChatBox {
    /// Open the input line, optionally with something already typed (`/` for a command).
    pub fn open_with(&mut self, text: &str) {
        self.open = true;
        self.input = text.to_string();
        self.grab_focus = true;
        self.recall = None;
        self.completion = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.input.clear();
        self.completion = None;
    }

    /// Draw the box in the bottom-left corner. `mentions` are the names worth highlighting.
    pub fn show(&mut self, ctx: &egui::Context, log: &VecDeque<ChatEntry>, now: f64, complete_ctx: &Context, mentions: &[(String, Mention)]) -> Outcome {
        let mut out = Outcome::default();
        let visible: Vec<&ChatEntry> = if self.open {
            log.iter().rev().take(LINES_OPEN).collect()
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
                    ui.add_space(4.0);
                    self.candidates_row(ui);
                    out.send = self.input_row(ui, complete_ctx);
                }
            });
        });
        out
    }

    /// The alternatives Tab is cycling through, current one highlighted.
    fn candidates_row(&self, ui: &mut egui::Ui) {
        let Some(c) = self.completion.as_ref().filter(|c| c.candidates.len() > 1 && c.applied == self.input) else { return };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            for (i, cand) in c.candidates.iter().enumerate().take(12) {
                let colour = if i == c.index { style::ACCENT } else { style::DIM };
                ui.label(egui::RichText::new(cand).small().color(style::c32(colour)));
            }
            if c.candidates.len() > 12 {
                ui.label(egui::RichText::new(format!("+{}", c.candidates.len() - 12)).small().color(style::c32(style::DIM)));
            }
        });
    }

    fn input_row(&mut self, ui: &mut egui::Ui, complete_ctx: &Context) -> Option<String> {
        let id = egui::Id::new("chat-input");
        let edit = egui::TextEdit::singleline(&mut self.input)
            .id(id)
            .desired_width(f32::INFINITY)
            .char_limit(MAX_CHAT_CHARS)
            .hint_text("message or /command")
            // Keep Tab for completion instead of moving focus.
            .lock_focus(true);
        let response = ui.add(edit);
        if std::mem::take(&mut self.grab_focus) {
            response.request_focus();
        }
        self.input.retain(|c| c != '\t');

        let pressed = |key| ui.input(|i| i.key_pressed(key));
        let mut edited = false;
        if pressed(egui::Key::Tab) {
            edited = self.complete(complete_ctx);
        } else if pressed(egui::Key::ArrowUp) || pressed(egui::Key::ArrowDown) {
            edited = self.recall(pressed(egui::Key::ArrowUp));
        }
        if edited {
            // Put the cursor at the end of what we just wrote.
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                let end = egui::text::CCursor::new(self.input.chars().count());
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                state.store(ui.ctx(), id);
            }
        }
        if response.lost_focus() && pressed(egui::Key::Enter) {
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

    /// Tab: complete the word being typed; pressing it again walks through the alternatives.
    fn complete(&mut self, ctx: &Context) -> bool {
        match self.completion.as_mut().filter(|c| c.applied == self.input) {
            Some(c) => c.index = (c.index + 1) % c.candidates.len(),
            None => {
                let candidates = complete::candidates(&self.input, ctx);
                if candidates.is_empty() {
                    self.completion = None;
                    return false;
                }
                self.completion = Some(Completion { base: self.input.clone(), candidates, index: 0, applied: String::new() });
            }
        }
        let c = self.completion.as_mut().expect("set above");
        self.input = complete::apply(&c.base, &c.candidates[c.index]);
        c.applied = self.input.clone();
        true
    }

    /// Up / Down: walk through what was sent before.
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
