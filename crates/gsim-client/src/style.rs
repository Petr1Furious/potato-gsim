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

/// A ring around `s` from the radius `from` to `to`, its colour running through `colors`
/// (evenly spaced between the two radii) without steps. Drawn once, however large: layers of
/// see-through discs would cost a screenful each when zoomed in.
fn gradient(s: (f32, f32), from: f32, to: f32, colors: &[Color]) {
    let around = sides(to) as usize;
    let rim = |level: usize| {
        let radius = from + (to - from) * level as f32 / (colors.len() - 1) as f32;
        (0..around).map(move |k| {
            let a = k as f32 / around as f32 * std::f32::consts::TAU;
            macroquad::models::Vertex::new(s.0 + a.cos() * radius, s.1 + a.sin() * radius, 0.0, 0.0, 0.0, colors[level])
        })
    };
    // One band at a time: there is a limit to how much goes to the card in one piece.
    for level in 0..colors.len() - 1 {
        let vertices: Vec<_> = rim(level).chain(rim(level + 1)).collect();
        let mut indices = Vec::with_capacity(around * 6);
        for k in 0..around {
            let (a, b) = (k as u16, ((k + 1) % around) as u16);
            let (c, d) = (a + around as u16, b + around as u16);
            indices.extend([a, b, c, b, d, c]);
        }
        macroquad::models::draw_mesh(&macroquad::models::Mesh { vertices, indices, texture: None });
    }
}

pub fn disc(x: f32, y: f32, r: f32, color: Color) {
    draw_poly(x, y, sides(r), r, 0.0, color);
}

pub fn ring(x: f32, y: f32, r: f32, thickness: f32, color: Color) {
    draw_poly_lines(x, y, sides(r), r, 0.0, thickness, color);
}

/// Part of a ring, from angle `a0` to `a1` (radians; screen y points down, so positive is
/// clockwise).
pub fn arc(x: f32, y: f32, r: f32, a0: f32, a1: f32, thickness: f32, color: Color) {
    let n = ((a1 - a0).abs() * r / 3.0).clamp(3.0, 96.0) as usize;
    let at = |k: usize| {
        let a = a0 + (a1 - a0) * k as f32 / n as f32;
        (x + r * a.cos(), y + r * a.sin())
    };
    for k in 0..n {
        let (p, q) = (at(k), at(k + 1));
        draw_line(p.0, p.1, q.0, q.1, thickness, color);
    }
}

/// A player's base: a glowing core in their colour inside a slowly turning shield whose
/// segments go dark as it loses health; under siege the shield flickers red. A fallen base is
/// a dark hulk among the scattered pieces of its shield.
pub fn base(s: (f32, f32), r: f32, colour: Color, health: f32, besieged: bool, slot: u32, ui: f32) {
    let time = get_time() as f32;
    let r = r.max(6.0 * ui).min(1.0e5);
    const SEGMENTS: usize = 12;
    let span = std::f32::consts::TAU / SEGMENTS as f32;
    let shield = r + 6.0 * ui;
    if health <= 0.0 {
        disc(s.0, s.1, r, Color::new(0.09, 0.09, 0.11, 1.0));
        ring(s.0, s.1, r, 1.5 * ui, Color::new(0.30, 0.30, 0.34, 1.0));
        // Some pieces are gone, the rest have drifted out a little; always the same ones.
        for k in 0..SEGMENTS {
            let h = (slot as usize * 7 + k * 5) % 11;
            if h < 4 {
                continue;
            }
            let a = k as f32 * span + 0.2 * (h as f32 - 5.0) / 5.0;
            let out = shield + (h as f32 - 4.0) * 1.2 * ui;
            arc(s.0, s.1, out, a, a + 0.55 * span, 2.0 * ui, alpha(mix(colour, DIM, 0.75), 0.45));
        }
        return;
    }
    let pulse = 0.5 + 0.5 * (time * 2.0 + slot as f32).sin();
    // A wide soft glow, then the core: dark at the rim, bright in the middle.
    for k in 1..=5 {
        disc(s.0, s.1, r + (4.0 + 5.0 * k as f32) * ui, alpha(colour, 0.035 + 0.015 * pulse));
    }
    disc(s.0, s.1, r, mix(BACKGROUND, colour, 0.25));
    for (k, f) in [(0.8, 0.45), (0.55, 0.7), (0.3, 1.0)] {
        disc(s.0, s.1, r * k, mix(BACKGROUND, colour, f));
    }
    disc(s.0, s.1, (r * 0.12).max(1.5 * ui), alpha(WHITE, 0.6 + 0.4 * pulse));
    // The shield: lit segments for the health left.
    let lit = (health.clamp(0.0, 1.0) * SEGMENTS as f32).ceil() as usize;
    let flicker = besieged && (time * 14.0).sin() > 0.0;
    let on = if flicker { EMBER } else { colour };
    let turn = time * 0.35;
    for k in 0..SEGMENTS {
        let a = turn + k as f32 * span;
        let c = if k < lit { alpha(on, 0.9) } else { alpha(colour, 0.12) };
        arc(s.0, s.1, shield, a, a + 0.78 * span, 2.5 * ui, c);
    }
    // A thin outer ring of ticks turning the other way.
    let ticks = shield + 5.0 * ui;
    for k in 0..24 {
        let a = -turn * 0.6 + k as f32 * span / 2.0;
        arc(s.0, s.1, ticks, a, a + 0.12, 1.0 * ui, alpha(colour, 0.35));
    }
}

/// A team's colour (in the order of [`gsim_proto::TEAM_NAMES`]).
pub fn team_colour(team: gsim_proto::TeamId) -> Color {
    const COLOURS: [Color; 8] = [
        Color::new(1.0, 0.42, 0.42, 1.0),
        Color::new(0.42, 0.66, 1.0, 1.0),
        Color::new(0.45, 0.92, 0.52, 1.0),
        Color::new(1.0, 0.82, 0.30, 1.0),
        Color::new(0.78, 0.52, 1.0, 1.0),
        Color::new(0.40, 0.92, 0.95, 1.0),
        Color::new(1.0, 0.62, 0.25, 1.0),
        Color::new(0.92, 0.92, 0.95, 1.0),
    ];
    COLOURS[(team.max(1) as usize - 1) % COLOURS.len()]
}

/// A shield's colour.
pub const SHIELD: Color = Color::new(0.62, 0.72, 1.0, 1.0);
/// A gravity well's.
pub const WELL: Color = Color::new(0.78, 0.45, 1.0, 1.0);

/// The mark around a crystal mine (radius `r` on screen): a breathing halo in the crystals'
/// colour, the crystals still in it circling it (dim once taken), and `held` (0 to 1) of the
/// way to the next one as an arc.
pub fn mine(s: (f32, f32), r: f32, left: u32, held: f32, ui: f32) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let time = get_time() as f32;
    let c = piece_colour(gsim_proto::PieceKind::Crystal);
    let ring = r.min(4000.0) + 9.0 * ui;
    let pulse = 0.75 + 0.25 * (time * 2.2).sin();
    for k in 1..=4 {
        self::ring(s.0, s.1, ring + 2.0 * k as f32 * ui, 2.5 * ui, alpha(c, 0.14 * pulse / k as f32));
    }
    self::ring(s.0, s.1, ring, 1.5 * ui, alpha(c, 0.7));
    let stock = gsim_proto::MINE_STOCK;
    for k in 0..stock {
        let a = time * 0.5 + k as f32 * TAU / stock as f32;
        let at = (s.0 + a.cos() * (ring + 7.0 * ui), s.1 + a.sin() * (ring + 7.0 * ui));
        if k < left {
            piece(at, gsim_proto::PieceKind::Crystal, time + k as f32 * 2.1, ui);
        } else {
            disc(at.0, at.1, 2.0 * ui, alpha(c, 0.25));
        }
    }
    if held > 0.0 {
        arc(s.0, s.1, ring + 15.0 * ui, -FRAC_PI_2, -FRAC_PI_2 + TAU * held.min(1.0), 3.0 * ui, c);
    }
}

/// A gravity well: spiral arms winding in, turning, `left` (1 to 0) of its time to go.
/// `reach`: how far out (on screen) it pulls as hard as an engine pushes.
pub fn well(s: (f32, f32), reach: f32, left: f32, ui: f32) {
    use std::f32::consts::TAU;
    let time = get_time() as f32;
    let fade = left.clamp(0.0, 1.0).sqrt();
    let outer = reach.clamp(24.0 * ui, 2000.0);
    disc(s.0, s.1, outer, alpha(WELL, 0.05 * fade));
    ring(s.0, s.1, outer, 1.0 * ui, alpha(WELL, 0.3 * fade));
    disc(s.0, s.1, outer * 0.35, alpha(WELL, 0.10 * fade));
    disc(s.0, s.1, 4.0 * ui, alpha(WHITE, 0.9 * fade));
    for arm in 0..4 {
        let mut prev: Option<(f32, f32)> = None;
        for k in 0..=24 {
            let t = k as f32 / 24.0;
            let a = arm as f32 * TAU / 4.0 + time * 2.5 + 3.0 * t;
            let rr = outer * (1.0 - t) + 4.0 * ui * t;
            let p = (s.0 + rr * a.cos(), s.1 + rr * a.sin());
            if let Some(q) = prev {
                draw_line(q.0, q.1, p.0, p.1, (1.0 + 1.5 * t) * ui, alpha(WELL, fade * (0.15 + 0.7 * t)));
            }
            prev = Some(p);
        }
    }
}

/// A ship jumping away (`age` 0 to 1): a flash, and a ring closing in on it.
pub fn warp(s: (f32, f32), colour: Color, age: f32, ui: f32) {
    let fade = (1.0 - age) * (1.0 - age);
    ring(s.0, s.1, (40.0 * (1.0 - age) + 3.0) * ui, 2.0 * ui, alpha(colour, fade));
    disc(s.0, s.1, 10.0 * ui * (1.0 - age), alpha(WHITE, 0.7 * fade));
}

/// A zone around `s` of radius `r` (screen pixels): a faint tint with a rim, brighter when
/// `lit`. However far in the view is zoomed: when the screen lies wholly inside, it is tinted
/// all over; when only the rim crosses it, the visible stretch of the rim is drawn.
pub fn zone(s: (f32, f32), r: f32, colour: Color, lit: bool, ui: f32) {
    let (w, h) = (screen_width(), screen_height());
    let (fill, rim, width) = if lit { (0.07, 0.7, 2.0 * ui) } else { (0.035, 0.35, 1.2 * ui) };
    // Nearest and farthest points of the screen from the centre.
    let near = ((s.0.clamp(0.0, w) - s.0).powi(2) + (s.1.clamp(0.0, h) - s.1).powi(2)).sqrt();
    let far = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|c: &(f32, f32)| ((c.0 - s.0).powi(2) + (c.1 - s.1).powi(2)).sqrt()).fold(0.0, f32::max);
    if r < near {
        return;
    }
    if r >= far {
        draw_rectangle(0.0, 0.0, w, h, alpha(colour, fill));
    } else {
        disc(s.0, s.1, r, alpha(colour, fill));
        ring(s.0, s.1, r, width, alpha(colour, rim));
    }
}

/// The rim of a zone far too big to draw whole, where it crosses the screen: straight there,
/// through `p`, with the inside towards `inward` (a unit vector), tinted.
pub fn zone_edge(p: (f32, f32), inward: (f32, f32), colour: Color, lit: bool, ui: f32) {
    let (fill, rim, width) = if lit { (0.07, 0.7, 2.0 * ui) } else { (0.035, 0.35, 1.2 * ui) };
    let reach = 2.0 * (screen_width() + screen_height());
    let along = (-inward.1 * reach, inward.0 * reach);
    let (a, b) = (vec2(p.0 - along.0, p.1 - along.1), vec2(p.0 + along.0, p.1 + along.1));
    let deep = vec2(inward.0 * reach, inward.1 * reach);
    draw_triangle(a, b, b + deep, alpha(colour, fill));
    draw_triangle(a, b + deep, a + deep, alpha(colour, fill));
    draw_line(a.x, a.y, b.x, b.y, width, alpha(colour, rim));
}

/// A ship's shield: a faint bubble around it with a bright rim, two highlights gliding round
/// it, breathing slowly. `seed` keeps neighbouring ships out of step.
pub fn ship_shield(s: (f32, f32), seed: u32, ui: f32) {
    use std::f32::consts::PI;
    let c = SHIELD;
    let time = get_time() as f32 + seed as f32 * 0.7;
    let r = (16.0 + 0.8 * (time * 2.0).sin()) * ui;
    disc(s.0, s.1, r, alpha(c, 0.06));
    ring(s.0, s.1, r, 1.2 * ui, alpha(c, 0.55));
    for k in 0..2 {
        let a = time * 1.3 + k as f32 * PI;
        arc(s.0, s.1, r, a, a + 0.9, 2.2 * ui, alpha(WHITE, 0.55));
    }
}

/// A shield breaking: its pieces flying apart and fading (`age` 0 to 1).
pub fn shield_break(s: (f32, f32), age: f32, ui: f32) {
    use std::f32::consts::TAU;
    let c = SHIELD;
    let r = (16.0 + 30.0 * age) * ui;
    for k in 0..8 {
        let a = k as f32 * TAU / 8.0 + 0.3 * age;
        arc(s.0, s.1, r, a, a + 0.45, (2.2 - 1.5 * age) * ui, alpha(c, (1.0 - age) * (1.0 - age)));
    }
}

pub fn piece_colour(kind: gsim_proto::PieceKind) -> Color {
    match kind {
        gsim_proto::PieceKind::Scrap => Color::new(0.90, 0.72, 0.50, 1.0),
        gsim_proto::PieceKind::Crystal => Color::new(0.55, 0.95, 1.0, 1.0),
    }
}

/// A piece floating free: scrap a tumbling shard of metal, a crystal a glinting diamond.
/// `spin` turns it (radians).
pub fn piece(s: (f32, f32), kind: gsim_proto::PieceKind, spin: f32, ui: f32) {
    use std::f32::consts::TAU;
    let c = piece_colour(kind);
    let at = |a: f32, r: f32| vec2(s.0 + r * ui * (spin + a).cos(), s.1 + r * ui * (spin + a).sin());
    match kind {
        gsim_proto::PieceKind::Scrap => {
            disc(s.0, s.1, 5.5 * ui, alpha(c, 0.10));
            // Lopsided, so that it is seen to tumble; lit on the side that faces up.
            let (p, q, r) = (at(0.0, 5.2), at(0.36 * TAU, 3.3), at(0.68 * TAU, 4.2));
            draw_triangle(p, q, r, mix(c, BACKGROUND, 0.35));
            draw_triangle(p, q, vec2(s.0, s.1), c);
            draw_triangle_lines(p, q, r, 0.8 * ui, alpha(WHITE, 0.35));
        }
        gsim_proto::PieceKind::Crystal => {
            disc(s.0, s.1, 9.0 * ui, alpha(c, 0.10));
            disc(s.0, s.1, 5.0 * ui, alpha(c, 0.14));
            let tilt = 0.15 * spin.sin();
            let tip = |a: f32, r: f32| vec2(s.0 + r * ui * (tilt + a).sin(), s.1 - r * ui * (tilt + a).cos());
            let (top, right, bottom, left) = (tip(0.0, 5.5), tip(0.25 * TAU, 3.0), tip(0.5 * TAU, 5.5), tip(0.75 * TAU, 3.0));
            draw_triangle(top, right, bottom, c);
            draw_triangle(top, left, bottom, mix(c, BACKGROUND, 0.3));
            // A glint now and then.
            let glint = (spin * 1.7).sin().max(0.0).powi(6);
            if glint > 0.02 {
                let (l, k) = ((3.0 + 6.0 * glint) * ui, alpha(WHITE, glint));
                draw_line(top.x - l, top.y, top.x + l, top.y, 1.0 * ui, k);
                draw_line(top.x, top.y - l, top.x, top.y + l, 1.0 * ui, k);
            }
        }
    }
}

/// A piece being taken: a spark that bursts outwards (`age` 0 to 1).
pub fn pickup(s: (f32, f32), kind: gsim_proto::PieceKind, age: f32, ui: f32) {
    let c = piece_colour(kind);
    let fade = (1.0 - age) * (1.0 - age);
    ring(s.0, s.1, (4.0 + 18.0 * age) * ui, (2.0 - 1.2 * age) * ui, alpha(c, fade));
    disc(s.0, s.1, (5.0 - 3.0 * age) * ui, alpha(WHITE, 0.8 * fade));
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::new(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1.0)
}

fn ramp(stops: &[Color], t: f32) -> Color {
    let x = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (x.floor() as usize).min(stops.len() - 2);
    mix(stops[i], stops[i + 1], x - i as f32)
}

/// What a body is, going by its mass alone (the ranges are the real ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Too light to pull itself round: asteroids, small moons.
    Lump,
    /// Up to two Earths.
    Rocky,
    /// Uranus, Neptune.
    IceGiant,
    /// Up to thirteen Jupiters.
    GasGiant,
    /// Too light to shine as a star does, up to 0.075 Suns.
    BrownDwarf,
    Star,
}

/// Each kind with its name and the mass (kg) it starts at.
pub const KINDS: [(Kind, &str, f64); 6] = [
    (Kind::Lump, "small body", 0.0),
    (Kind::Rocky, "rocky world", 1.0e20),
    (Kind::IceGiant, "ice giant", 1.2e25),
    (Kind::GasGiant, "gas giant", 3.0e26),
    (Kind::BrownDwarf, "brown dwarf", 2.5e28),
    (Kind::Star, "star", 1.5e29),
];

pub fn kind(mass: f64) -> Kind {
    KINDS.iter().rev().find(|k| mass.abs() >= k.2).map_or(Kind::Lump, |k| k.0)
}

pub fn is_star(mass: f64) -> bool {
    kind(mass) == Kind::Star
}

/// Colour tells mass, and with it what a body is: grey and tan for rock, pale blue for ice
/// giants, sand and rust for gas giants, a dull dark red for brown dwarfs, and stars from red
/// through orange, yellow and white to blue-white, as real ones are. It changes smoothly all
/// the way, so that no two bodies nearly alike look unlike. Negative masses run from dusky
/// violet to hot pink.
pub fn mass_color(mass: f64) -> Color {
    // (log10 of the mass in kg, colour)
    // Each kind has colours of its own from its lightest to its heaviest, and one runs into
    // the next over a fifth of a power of ten.
    const POSITIVE: [(f32, Color); 15] = [
        (19.0, Color::new(0.42, 0.40, 0.40, 1.0)),
        (19.9, Color::new(0.48, 0.46, 0.45, 1.0)),
        (20.1, Color::new(0.52, 0.52, 0.56, 1.0)),
        (24.95, Color::new(0.82, 0.72, 0.56, 1.0)),
        (25.2, Color::new(0.66, 0.88, 0.90, 1.0)),
        (26.4, Color::new(0.42, 0.62, 0.96, 1.0)),
        (26.6, Color::new(0.94, 0.86, 0.66, 1.0)),
        (28.3, Color::new(0.84, 0.52, 0.32, 1.0)),
        (28.5, Color::new(0.50, 0.24, 0.30, 1.0)),
        (29.15, Color::new(0.76, 0.27, 0.22, 1.0)),
        (29.5, Color::new(1.0, 0.44, 0.28, 1.0)),
        (30.0, Color::new(1.0, 0.72, 0.44, 1.0)),
        (30.3, Color::new(1.0, 0.94, 0.74, 1.0)),
        (30.65, Color::new(1.0, 1.0, 0.96, 1.0)),
        (31.2, Color::new(0.70, 0.80, 1.0, 1.0)),
    ];
    const NEGATIVE: [Color; 3] = [Color::new(0.42, 0.30, 0.58, 1.0), Color::new(0.78, 0.38, 0.86, 1.0), Color::new(1.0, 0.40, 0.80, 1.0)];
    let log = mass.abs().max(1.0).log10() as f32;
    if mass < 0.0 {
        return ramp(&NEGATIVE, (log - 21.0) / 6.0);
    }
    let next = POSITIVE.iter().position(|stop| stop.0 > log).unwrap_or(POSITIVE.len() - 1).max(1);
    let (a, b) = (POSITIVE[next - 1], POSITIVE[next]);
    mix(a.1, b.1, ((log - a.0) / (b.0 - a.0)).clamp(0.0, 1.0))
}

/// How much a body shines by itself, 0 to 1: nothing lighter than a brown dwarf does, a
/// brown dwarf the more the heavier it is (up to a third), and stars on from there.
pub fn shine(mass: f64) -> f32 {
    let log = mass.max(1.0).log10() as f32;
    let (dwarf, star) = (KINDS[4].2.log10() as f32, KINDS[5].2.log10() as f32);
    if log < star { 0.33 * ((log - dwarf) / (star - dwarf)).max(0.0) } else { 0.33 + 0.67 * ((log - star) / 1.8).min(1.0) }
}

/// Where a body sits between the lightest and the heaviest there are, 0 to 1.
pub fn heft(mass: f64) -> f32 {
    ((mass.abs().max(1.0).log10() as f32 - 20.0) / 10.5).clamp(0.0, 1.0)
}

/// Cheap per-body pseudo-random number in `0..1` (stable from frame to frame).
fn hash01(slot: u32, salt: u32) -> f32 {
    let mut h = slot.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65536.0
}

/// Bands across a disc, as on a giant planet: a few that a body keeps from frame to frame,
/// each darker or lighter than what is under it, narrowing towards the poles as seen on a
/// sphere. Where one ends and the next begins does not depend on how large the disc is drawn.
fn bands(s: (f32, f32), r: f32, slot: u32, strength: f32, tint: impl Fn(f32) -> Color) {
    let count = 5 + (hash01(slot, 40) * 6.0) as u32;
    // Half the width of the disc at a height, a hair inside its edge.
    let half = |y: f32| (r * r - y * y).max(0.0).sqrt() * 0.995;
    let edge = |band: u32| -r * (std::f32::consts::FRAC_PI_2 * (1.0 - 2.0 * band as f32 / count as f32)).sin();
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for band in 0..count {
        let mut color = tint(if band % 2 == 0 { 0.6 } else { 1.3 });
        color.a *= strength * (0.35 + 0.65 * hash01(slot, 41 + band));
        // Strips thin enough for their straight ends to follow the circle.
        let (top, bottom) = (edge(band), edge(band + 1));
        let strips = ((bottom - top) / 2.0).ceil().clamp(1.0, 64.0) as usize;
        for k in 0..strips {
            let (from, to) = (top + (bottom - top) * k as f32 / strips as f32, top + (bottom - top) * (k + 1) as f32 / strips as f32);
            let first = vertices.len() as u16;
            for (x, y) in [(-half(from), from), (half(from), from), (half(to), to), (-half(to), to)] {
                vertices.push(macroquad::models::Vertex::new(s.0 + x, s.1 + y, 0.0, 0.0, 0.0, color));
            }
            indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
        }
    }
    macroquad::models::draw_mesh(&macroquad::models::Mesh { vertices, indices, texture: None });
}

/// A body at screen position `s` with on-screen radius `r`. Far away it is a dot, larger
/// and, for those that shine, with more of a glow the heavier it is; closer it is drawn as
/// what its mass makes it (see [`Kind`]). `light` points from the body to whatever lights it.
pub fn body(s: (f32, f32), r: f32, mass: f64, slot: u32, fade: f32, ui: f32, light: (f32, f32)) {
    let base = mass_color(mass);
    let shade = |k: f32, a: f32| Color::new((base.r * k).min(1.0), (base.g * k).min(1.0), (base.b * k).min(1.0), a * fade);
    let (shine, heft) = (if mass > 0.0 { shine(mass) } else { 0.0 }, heft(mass));
    if r < 1.3 * ui {
        let dot = (0.9 + 1.5 * heft) * ui;
        if shine > 0.0 {
            for k in 0..5 {
                disc(s.0, s.1, dot * (1.0 + (0.6 + 2.4 * shine) * (1.0 - k as f32 / 5.0).powi(2)), shade(1.0, 0.03 + 0.05 * shine));
            }
        }
        draw_poly(s.0, s.1, 10, dot, 0.0, shade(1.0, 0.75 + 0.25 * heft));
        return;
    }
    let r = r.min(1.0e5);
    // Towards the light, by a fraction of the radius.
    let lit = |k: f32| (s.0 + light.0 * r * k, s.1 + light.1 * r * k);
    if shine > 0.0 {
        // Brown dwarfs and stars are one family: the heavier, the wider and stronger the glow
        // around them and the brighter the middle, while the dark bands of a brown dwarf fade
        // away over the lightest stars.
        let star = ((shine - 0.33) / 0.15).clamp(0.0, 1.0);
        // The light around it, falling off away from the body.
        let reach = r * (0.25 + 0.9 * shine) + 8.0 * shine * ui;
        let strength = 0.35 + 0.5 * shine;
        let falling: [Color; 7] = std::array::from_fn(|k| shade(1.0, strength * (1.0 - k as f32 / 6.0).powf(2.5)));
        gradient(s, r * 0.98, r + reach, &falling);
        // The body: brightest in the middle, darker at the limb.
        let limb = |t: f32| shade(0.62 + 0.3 * star + (0.3 + 0.2 * star) * (1.0 - t), 1.0);
        gradient(s, 0.0, r, &[limb(0.0), limb(0.25), limb(0.6), limb(1.0)]);
        if r >= 5.0 * ui && star < 1.0 {
            bands(s, r * 0.98, slot, 0.5 * (1.0 - star), |k| shade(0.45 * k, 1.0));
        }
        return;
    }
    if r < 5.0 * ui {
        disc(s.0, s.1, r, shade(1.0, 1.0));
        return;
    }
    let kind = if mass < 0.0 { Kind::Rocky } else { kind(mass) };
    if kind == Kind::Lump {
        // Not round: a fan of wedges of uneven reach, and a lighter one inside it towards the light.
        let corners = 9;
        let reach = |k: usize, scale: f32, centre: (f32, f32)| {
            let a = k as f32 / corners as f32 * std::f32::consts::TAU + hash01(slot, 5) * 6.0;
            let d = r * scale * (0.68 + 0.42 * hash01(slot, 20 + (k % corners) as u32));
            vec2(centre.0 + a.cos() * d, centre.1 + a.sin() * d)
        };
        for (scale, centre, color) in [(1.0, s, shade(0.6, 1.0)), (0.8, lit(0.1), shade(0.85, 1.0)), (0.55, lit(0.22), shade(1.05, 1.0))] {
            for k in 0..corners {
                draw_triangle(vec2(centre.0, centre.1), reach(k, scale, centre), reach(k + 1, scale, centre), color);
            }
        }
        return;
    }
    // A lit sphere: dark limb, brighter towards the side the light comes from.
    disc(s.0, s.1, r, shade(0.55, 1.0));
    let (x, y) = lit(0.10);
    disc(x, y, r * 0.88, shade(0.80, 1.0));
    let (x, y) = lit(0.20);
    disc(x, y, r * 0.72, shade(1.0, 1.0));
    match kind {
        Kind::IceGiant => bands(s, r, slot, 0.16, |k| shade(k, 1.0)),
        Kind::GasGiant => {
            bands(s, r, slot, 0.42, |k| shade(k, 1.0));
            if r > 14.0 * ui && hash01(slot, 60) < 0.6 {
                // A storm: a flattened spot at some latitude.
                let (a, d) = (hash01(slot, 61) * std::f32::consts::TAU, 0.25 + 0.3 * hash01(slot, 62));
                let (x, y) = (s.0 + a.cos() * d * r, s.1 + a.sin() * d * r * 0.7);
                draw_ellipse(x, y, 0.16 * r, 0.09 * r, 0.0, shade(1.35, 0.55));
            }
        }
        _ if r > 14.0 * ui => {
            // Rock: craters and darker plains, the same every frame for a given body.
            let marks = 4 + (hash01(slot, 0) * 5.0) as u32;
            for k in 0..marks {
                let a = hash01(slot, 1 + 3 * k) * std::f32::consts::TAU;
                let d = (0.1 + 0.6 * hash01(slot, 2 + 3 * k)) * r;
                let size = (0.05 + 0.14 * hash01(slot, 3 + 3 * k)) * r;
                let at = (s.0 + a.cos() * d, s.1 + a.sin() * d);
                // Dark, with the floor lit on the side away from the light.
                disc(at.0, at.1, size, shade(0.45, 0.5));
                disc(at.0 - light.0 * size * 0.25, at.1 - light.1 * size * 0.25, size * 0.6, shade(0.8, 0.25));
            }
        }
        _ => {}
    }
    if r > 14.0 * ui {
        let (x, y) = lit(0.38);
        disc(x, y, r * 0.3, shade(1.3, 0.2));
    }
    ring(s.0, s.1, r, ui, shade(1.25, 0.35));
}

/// The way light falls when nothing in particular lights a body: from the upper left.
pub const NO_LIGHT: (f32, f32) = (-0.707, -0.707);

/// Which way the light comes from for a body at `s`, with a star at `star` (both on screen).
pub fn light_from(s: (f32, f32), star: Option<(f32, f32)>) -> (f32, f32) {
    match star.map(|at| (at.0 - s.0, at.1 - s.1)) {
        Some((dx, dy)) if dx * dx + dy * dy > 1.0 => {
            let d = (dx * dx + dy * dy).sqrt();
            (dx / d, dy / d)
        }
        _ => NO_LIGHT,
    }
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

/// The key to how bodies are drawn: each kind with its colours and the masses it covers.
pub fn mass_legend(ui: &mut egui::Ui) {
    let row = |ui: &mut egui::Ui, name: &str, range: String, color: &dyn Fn(f32) -> Color| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 9.0), egui::Sense::hover());
            for i in 0..22 {
                let x = rect.left() + rect.width() * i as f32 / 22.0;
                let cell = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + rect.width() / 22.0 + 0.5, rect.bottom()));
                ui.painter().rect_filled(cell, 0.0, c32(color(i as f32 / 21.0)));
            }
            ui.label(egui::RichText::new(name).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(range).small().color(c32(DIM)));
            });
        });
    };
    let short = |kg: f64| format!("{kg:.1e}").replace(".0e", "e");
    for (k, (_, name, from)) in KINDS.iter().enumerate() {
        // The lightest and the heaviest kinds have no end: shown over a few powers of ten.
        let (lo, hi) = (from.max(1.0e18).log10(), KINDS.get(k + 1).map_or(31.2, |next| next.2.log10()));
        let range = match (k, KINDS.get(k + 1)) {
            (0, Some(next)) => format!("under {} kg", short(next.2)),
            (_, Some(next)) => format!("{} to {} kg", short(*from), short(next.2)),
            (_, None) => format!("over {} kg", short(*from)),
        };
        row(ui, name, range, &|t| mass_color(10f64.powf(lo + (hi - lo) * t as f64)));
    }
    row(ui, "negative mass", String::new(), &|t| mass_color(-(10f64.powf(21.0 + 6.0 * t as f64))));
    ui.add_space(4.0);
}

// --- map labels ------------------------------------------------------------------------------

/// How much a label matters when two of them land on the same spot.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    BodyName,
    Player,
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
        self.items.sort_by_key(|item| std::cmp::Reverse(item.rank));
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
