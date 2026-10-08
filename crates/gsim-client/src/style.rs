//! Look and feel in one place: fonts, palette, shape and text helpers for the world view,
//! and the few custom HUD widgets.

use egui_macroquad::egui;
use macroquad::prelude::*;
use std::cell::RefCell;
use std::sync::Arc;

const FONT_UI: &[u8] = include_bytes!("../assets/rajdhani-Rajdhani-Medium.ttf");
const FONT_MONO: &[u8] = include_bytes!("../assets/sharetechmono-ShareTechMono-Regular.ttf");

pub const BACKGROUND: Color = Color::new(0.020, 0.027, 0.047, 1.0);
pub const ACCENT: Color = Color::new(0.37, 0.83, 1.0, 1.0);
pub const GOLD: Color = Color::new(1.0, 0.78, 0.24, 1.0);
pub const EMBER: Color = Color::new(1.0, 0.47, 0.35, 1.0);
pub const GOOD: Color = Color::new(0.45, 0.95, 0.60, 1.0);
pub const TEXT: Color = Color::new(0.84, 0.88, 0.94, 1.0);
pub const DIM: Color = Color::new(0.50, 0.56, 0.66, 1.0);
pub const OTHER_SHIP: Color = Color::new(0.51, 0.78, 1.0, 1.0);

thread_local! {
    static FONT: RefCell<Option<Font>> = const { RefCell::new(None) };
}

pub fn alpha(c: Color, a: f32) -> Color {
    Color::new(c.r, c.g, c.b, c.a * a)
}

pub fn rgb(c: [u8; 3]) -> Color {
    Color::from_rgba(c[0], c[1], c[2], 255)
}

pub fn c32(c: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied((c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8, (c.a * 255.0) as u8)
}

/// Load fonts and apply the HUD theme. Call once after the window exists.
pub fn init() {
    if let Ok(font) = load_ttf_font_from_bytes(FONT_UI) {
        FONT.with(|f| *f.borrow_mut() = Some(font));
    }
    egui_macroquad::cfg(|ctx| {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert("ui".into(), Arc::new(egui::FontData::from_static(FONT_UI)));
        fonts.font_data.insert("mono".into(), Arc::new(egui::FontData::from_static(FONT_MONO)));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "ui".into());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "mono".into());
        ctx.set_fonts(fonts);

        let mut style = (*ctx.style()).clone();
        use egui::{FontFamily::*, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(24.0, Proportional)),
            (TextStyle::Body, FontId::new(16.0, Proportional)),
            (TextStyle::Button, FontId::new(16.0, Proportional)),
            (TextStyle::Small, FontId::new(13.0, Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 4.0);
        // Text on screen is something to read, not to drag across.
        style.interaction.selectable_labels = false;
        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(c32(TEXT));
        v.window_fill = egui::Color32::from_rgba_unmultiplied(8, 12, 20, 235);
        v.window_stroke = egui::Stroke::new(1.0, c32(alpha(ACCENT, 0.35)));
        v.window_corner_radius = egui::CornerRadius::same(3);
        v.window_shadow = egui::Shadow::NONE;
        v.popup_shadow = egui::Shadow::NONE;
        v.selection.bg_fill = c32(alpha(ACCENT, 0.35));
        v.selection.stroke = egui::Stroke::new(1.0, c32(ACCENT));
        v.extreme_bg_color = egui::Color32::from_rgb(4, 7, 12);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = egui::CornerRadius::same(2);
        }
        v.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(22, 30, 44);
        v.widgets.inactive.bg_fill = egui::Color32::from_rgb(22, 30, 44);
        v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(32, 46, 66);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, c32(ACCENT));
        v.widgets.active.weak_bg_fill = c32(alpha(ACCENT, 0.4));
        // Window title bars.
        v.widgets.open.weak_bg_fill = egui::Color32::from_rgb(14, 20, 32);
        v.widgets.open.bg_fill = egui::Color32::from_rgb(14, 20, 32);
        ctx.set_style(style);
    });
}

// --- world view ------------------------------------------------------------------------------

fn text_params(size: f32, color: Color, font: Option<&Font>) -> TextParams<'_> {
    TextParams { font, font_size: size.floor().max(1.0) as u16, color, ..Default::default() }
}

/// Text with its baseline at `y`, left edge at `x`.
pub fn text(s: &str, x: f32, y: f32, size: f32, color: Color) {
    FONT.with(|f| {
        draw_text_ex(s, x.round(), y.round(), text_params(size, color, f.borrow().as_ref()));
    });
}

/// Text horizontally centred on `x`, baseline at `y`.
pub fn centered(s: &str, x: f32, y: f32, size: f32, color: Color) {
    FONT.with(|f| {
        let font = f.borrow();
        let w = measure_text(s, font.as_ref(), size.floor().max(1.0) as u16, 1.0).width;
        draw_text_ex(s, (x - 0.5 * w).round(), y.round(), text_params(size, color, font.as_ref()));
    });
}

/// Enough segments that the edge looks round at any size.
fn sides(r: f32) -> u8 {
    (16.0 + 0.5 * r).clamp(20.0, 255.0) as u8
}

pub fn disc(x: f32, y: f32, r: f32, color: Color) {
    draw_poly(x, y, sides(r), r, 0.0, color);
}

pub fn ring(x: f32, y: f32, r: f32, thickness: f32, color: Color) {
    draw_poly_lines(x, y, sides(r), r, 0.0, thickness, color);
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::new(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1.0)
}

fn ramp(stops: &[Color], t: f32) -> Color {
    let x = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (x.floor() as usize).min(stops.len() - 2);
    mix(stops[i], stops[i + 1], x - i as f32)
}

pub fn is_star(mass: f64) -> bool {
    mass > 1.0e29
}

/// Colour tells mass (size barely does: radius grows with the cube root). Light bodies are
/// dim slate, through ice blue and pale sand to amber and ember for the heaviest; stars are
/// white-hot. Negative masses run from dusky violet to hot pink.
pub fn mass_color(mass: f64) -> Color {
    const POSITIVE: [Color; 5] = [
        Color::new(0.38, 0.44, 0.54, 1.0),
        Color::new(0.50, 0.74, 0.90, 1.0),
        Color::new(0.93, 0.90, 0.78, 1.0),
        Color::new(0.98, 0.70, 0.36, 1.0),
        Color::new(0.95, 0.40, 0.28, 1.0),
    ];
    const NEGATIVE: [Color; 3] = [Color::new(0.42, 0.30, 0.58, 1.0), Color::new(0.78, 0.38, 0.86, 1.0), Color::new(1.0, 0.40, 0.80, 1.0)];
    if is_star(mass) {
        return Color::new(1.0, 0.95, 0.78, 1.0);
    }
    // 1e21 kg .. 1e27 kg on a log scale.
    let t = ((mass.abs().max(1.0).log10() - 21.0) / 6.0) as f32;
    if mass < 0.0 {
        ramp(&NEGATIVE, t)
    } else {
        ramp(&POSITIVE, t)
    }
}

/// Cheap per-body pseudo-random number in `0..1` (stable from frame to frame).
fn hash01(slot: u32, salt: u32) -> f32 {
    let mut h = slot.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65536.0
}

/// A body at screen position `s` with on-screen radius `r`. Far away it is a dot; closer it
/// gains shading, a rim and a few surface features.
pub fn body(s: (f32, f32), r: f32, mass: f64, slot: u32, fade: f32, ui: f32) {
    let base = mass_color(mass);
    let shade = |k: f32, a: f32| Color::new((base.r * k).min(1.0), (base.g * k).min(1.0), (base.b * k).min(1.0), a * fade);
    if r < 1.3 * ui {
        // Heavier dots are a touch bigger and brighter, so mass reads even when zoomed out.
        let t = ((mass.abs().max(1.0).log10() - 21.0) / 6.0).clamp(0.0, 1.0) as f32;
        let dot = if is_star(mass) { 2.2 } else { 0.9 + 0.7 * t };
        draw_poly(s.0, s.1, 10, dot * ui, 0.0, shade(1.0, 0.75 + 0.25 * t));
        return;
    }
    let r = r.min(1.0e5);
    if is_star(mass) {
        // Corona.
        let glow = |a: f32| Color::new(1.0, 0.72, 0.30, a * fade);
        disc(s.0, s.1, r * 1.9 + 6.0 * ui, glow(0.07));
        disc(s.0, s.1, r * 1.35 + 3.0 * ui, glow(0.14));
        disc(s.0, s.1, r, shade(0.92, 1.0));
        disc(s.0, s.1, r * 0.82, shade(1.08, 1.0));
        return;
    }
    if r < 5.0 * ui {
        disc(s.0, s.1, r, shade(1.0, 1.0));
        return;
    }
    // A lit sphere: dark limb, brighter towards an off-centre highlight.
    let lit = |k: f32| (s.0 - r * k, s.1 - r * k);
    disc(s.0, s.1, r, shade(0.62, 1.0));
    let (x, y) = lit(0.07);
    disc(x, y, r * 0.90, shade(0.82, 1.0));
    let (x, y) = lit(0.14);
    disc(x, y, r * 0.74, shade(1.0, 1.0));
    if r > 14.0 * ui {
        // Surface marks, the same every frame for a given body.
        let marks = 3 + (hash01(slot, 0) * 3.0) as u32;
        for k in 0..marks {
            let a = hash01(slot, 1 + 3 * k) * std::f32::consts::TAU;
            let d = (0.1 + 0.5 * hash01(slot, 2 + 3 * k)) * r;
            let size = (0.06 + 0.12 * hash01(slot, 3 + 3 * k)) * r;
            disc(s.0 + a.cos() * d, s.1 + a.sin() * d, size, shade(0.55, 0.45));
        }
        let (x, y) = lit(0.28);
        disc(x, y, r * 0.32, shade(1.25, 0.25));
    }
    ring(s.0, s.1, r, ui, shade(1.25, 0.35));
}

/// Scale bar: a ruler of a round length. Returns nothing; draws centred on `x`.
pub fn scale_bar(x: f32, y: f32, metres_per_pixel: f64, label: impl Fn(f64) -> String, ui: f32) {
    let want = metres_per_pixel * 140.0 * ui as f64;
    let pow = 10f64.powf(want.log10().floor());
    let nice = [5.0, 2.0, 1.0].into_iter().map(|k| k * pow).find(|l| *l <= want).unwrap_or(pow);
    let half = (nice / metres_per_pixel) as f32 * 0.5;
    let c = alpha(DIM, 0.9);
    draw_line(x - half, y, x + half, y, ui, c);
    for end in [x - half, x + half] {
        draw_line(end, y - 3.0 * ui, end, y + 3.0 * ui, ui, c);
    }
    centered(&label(nice), x, y - 6.0 * ui, 12.0 * ui, c);
}

// --- HUD widgets -----------------------------------------------------------------------------

/// Borderless translucent panel.
pub fn panel() -> egui::Frame {
    egui::Frame::NONE.fill(egui::Color32::from_rgba_unmultiplied(6, 10, 18, 190)).inner_margin(10.0).corner_radius(3.0)
}

/// Small spaced-out caption, e.g. `caption(ui, "DELTA-V")`.
pub fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(c32(DIM)).extra_letter_spacing(1.5));
}

/// Slim segmented gauge with a caption on the left and a value on the right.
pub fn gauge(ui: &mut egui::Ui, label: &str, value: &str, frac: f32, color: Color) {
    ui.horizontal(|ui| {
        caption(ui, label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).small());
        });
    });
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 5.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 1.0, egui::Color32::from_rgb(24, 32, 46));
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    painter.rect_filled(fill, 1.0, c32(color));
    // Ten segments.
    for i in 1..10 {
        let x = rect.left() + rect.width() * i as f32 / 10.0;
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, egui::Color32::from_rgb(6, 10, 18)));
    }
    ui.add_space(3.0);
}

/// Section header inside a window.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    caption(ui, title);
    ui.add_space(1.0);
}

/// A keyboard shortcut, pushed to the right edge of its row.
pub fn key_hint(ui: &mut egui::Ui, key: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(egui::RichText::new(key).monospace().color(c32(DIM)));
    });
}

/// A switch with its keyboard shortcut right-aligned.
pub fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str, key: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let checkbox = ui.checkbox(value, label);
        key_hint(ui, key);
        checkbox
    })
    .inner
}

/// A line of the controls reference: what it does on the left, the keys on the right.
pub fn key_row(ui: &mut egui::Ui, what: &str, key: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(what).color(c32(DIM)));
        key_hint(ui, key);
    });
}

/// The colour-to-mass key: a strip of the ramp with a few labelled masses under it.
pub fn mass_legend(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::hover());
    let painter = ui.painter();
    // Positive ramp over most of the strip, then a star swatch and the negative ramp.
    let steps = 48;
    let ramp_w = rect.width() * 0.62;
    for i in 0..steps {
        let t = i as f64 / (steps - 1) as f64;
        let x = rect.left() + ramp_w * i as f32 / steps as f32;
        let cell = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + ramp_w / steps as f32 + 0.5, rect.bottom()));
        painter.rect_filled(cell, 0.0, c32(mass_color(10f64.powf(21.0 + 6.0 * t))));
    }
    let star = egui::Rect::from_min_max(egui::pos2(rect.left() + rect.width() * 0.66, rect.top()), egui::pos2(rect.left() + rect.width() * 0.74, rect.bottom()));
    painter.rect_filled(star, 0.0, c32(mass_color(2.0e30)));
    let neg_left = rect.left() + rect.width() * 0.78;
    let neg_w = rect.right() - neg_left;
    for i in 0..16 {
        let t = i as f64 / 15.0;
        let x = neg_left + neg_w * i as f32 / 16.0;
        let cell = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + neg_w / 16.0 + 0.5, rect.bottom()));
        painter.rect_filled(cell, 0.0, c32(mass_color(-(10f64.powf(21.0 + 6.0 * t)))));
    }
    let font = egui::FontId::new(11.0, egui::FontFamily::Proportional);
    let label = |x: f32, align: egui::Align2, text: &str| {
        painter.text(egui::pos2(x, rect.bottom() + 3.0), align, text, font.clone(), c32(DIM));
    };
    label(rect.left(), egui::Align2::LEFT_TOP, "1e21 kg");
    label(rect.left() + ramp_w * 0.5, egui::Align2::CENTER_TOP, "1e24");
    label(rect.left() + ramp_w, egui::Align2::RIGHT_TOP, "1e27");
    label(star.center().x, egui::Align2::CENTER_TOP, "star");
    label(rect.right(), egui::Align2::RIGHT_TOP, "negative");
    ui.add_space(16.0);
}

// --- map labels ------------------------------------------------------------------------------

/// How much a label matters when two of them land on the same spot.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    BodyName,
    Pilot,
    Approach,
    Target,
    Selection,
}

/// One label: one or more centred lines that belong together.
struct Label {
    lines: Vec<String>,
    x: f32,
    /// Baseline of the first line.
    y: f32,
    size: f32,
    color: Color,
    rank: Rank,
}

/// Distance between the baselines of consecutive lines, in units of the font size.
const LINE_STEP: f32 = 1.0;

/// Collects the frame's map labels so that, where they overlap, the less important one
/// fades out instead of the two becoming an unreadable smear.
#[derive(Default)]
pub struct Labels {
    items: Vec<Label>,
}

impl Labels {
    /// Single line centred on `x` with its baseline at `y`.
    pub fn push(&mut self, text: impl Into<String>, x: f32, y: f32, size: f32, color: Color, rank: Rank) {
        self.block(vec![text.into()], x, y, size, color, rank);
    }

    /// Several lines stacked downwards from `y`. They fade together, as one label.
    pub fn block(&mut self, lines: Vec<String>, x: f32, y: f32, size: f32, color: Color, rank: Rank) {
        if !lines.is_empty() {
            self.items.push(Label { lines, x, y, size, color, rank });
        }
    }

    pub fn draw(mut self) {
        // Most important first (stable, so equal ranks keep their order); each label fades by
        // how much of it the ones before it cover.
        self.items.sort_by(|a, b| b.rank.cmp(&a.rank));
        let boxes: Vec<Rect> = FONT.with(|f| {
            let font = f.borrow();
            self.items
                .iter()
                .map(|l| {
                    let px = l.size.floor().max(1.0) as u16;
                    let w = l.lines.iter().map(|t| measure_text(t, font.as_ref(), px, 1.0).width).fold(0.0, f32::max);
                    let pad = 0.25 * l.size;
                    let h = l.size * (1.25 + LINE_STEP * (l.lines.len() - 1) as f32);
                    Rect::new(l.x - 0.5 * w - pad, l.y - l.size, w + 2.0 * pad, h)
                })
                .collect()
        });
        for (i, label) in self.items.iter().enumerate() {
            let own = boxes[i];
            let covered = (0..i)
                .filter_map(|k| own.intersect(boxes[k]))
                .map(|hit| hit.w * hit.h / (own.w * own.h).max(1.0))
                .fold(0.0f32, f32::max);
            let visible = (1.0 - 3.0 * covered).clamp(0.06, 1.0);
            for (n, line) in label.lines.iter().enumerate() {
                centered(line, label.x, label.y + n as f32 * LINE_STEP * label.size, label.size, alpha(label.color, visible));
            }
        }
    }
}
