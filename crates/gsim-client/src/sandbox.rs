//! Large-scale single player: a world of up to hundreds of thousands of bodies run by
//! `gsim-swarm`, watched through a free camera and changed with a handful of tools. There is
//! no ship, no server, no score; time runs at whatever pace is asked for.

use crate::density::{ColorMode, Density, Flash};
use crate::fmt;
use crate::game::{Outcome, View, LABEL, PICK_RADIUS_PX};
use crate::settings::{Camera, Settings};
use crate::style::{self, Rank};
use egui_macroquad::egui;
use gsim_swarm::runner::{Command, Heavy, Runner, Stats};
use gsim_swarm::scenario::{self, radius_from_mass, Setup, Structure};
use gsim_swarm::{Bodies, Merge};
use macroquad::prelude::*;
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

const G: f64 = 6.67430e-11;
/// Bulk densities (kg/m^3) used to size new bodies: rock, and star above a tenth of a Sun.
const ROCK: f64 = 5514.0;

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Body(u32),
    Free,
}

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Look,
    Place,
    Spray,
    Drop,
    Erase,
    Shatter,
}

impl Tool {
    const ALL: [(Tool, &'static str, KeyCode); 6] = [
        (Tool::Look, "Look", KeyCode::Key1),
        (Tool::Place, "Place", KeyCode::Key2),
        (Tool::Spray, "Spray", KeyCode::Key3),
        (Tool::Drop, "Drop", KeyCode::Key4),
        (Tool::Erase, "Erase", KeyCode::Key5),
        (Tool::Shatter, "Shatter", KeyCode::Key6),
    ];
}

/// What the tools are set to.
struct Kit {
    /// Mass of a placed or sprayed body (kg).
    mass: f64,
    /// What new bodies are made of (kg/m³): it sets how large they are.
    density: f64,
    /// Brush radius on screen (pixels, before interface scaling).
    brush: f32,
    /// Bodies sprayed per second.
    rate: f32,
    structure: Structure,
    /// Bodies in a dropped structure, and its total mass.
    count: f64,
    bulk: f64,
    pieces: f64,
    violence: f64,
}

/// What the simulation thread last published, copied out under its lock.
struct Seen {
    clock: gsim_swarm::runner::Clock,
    time: f64,
    pace: f64,
    theta: f32,
    stats: Stats,
    heaviest: Vec<Heavy>,
    watch: Vec<u32>,
}

/// A body as drawn this frame.
#[derive(Clone, Copy)]
struct Picked {
    id: u32,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    mass: f64,
    radius: f64,
}

/// A left-button press that has not been released yet.
struct Press {
    /// Where it landed, relative to whatever the camera follows.
    at: (f64, f64),
    moved: f32,
}

/// Time buttons: simulated seconds per real second.
const PACES: [(f64, &str); 7] = [(0.0, "||"), (3600.0, "1h"), (21_600.0, "6h"), (86_400.0, "1d"), (172_800.0, "2d"), (345_600.0, "4d"), (691_200.0, "8d")];
/// Holding a pace key doubles or halves the pace this many times a second.
const PACE_KEY_RATE: f64 = 1.5;
/// Seconds a merge flash lasts.
const FLASH_SECONDS: f32 = 0.6;
/// The keyboard moves the view this many screen heights a second.
const PAN_RATE: f64 = 0.9;
/// A new body thrown with the mouse is shown where it will be this many seconds on.
const PREVIEW_SECONDS: f64 = 4.0;

pub struct Sandbox {
    runner: Runner,
    scenario: String,
    view: View,
    view_ready: bool,
    zoom_pending: f32,
    /// Camera offset from whatever it follows.
    offset: (f64, f64),
    last_target: Target,
    last_target_pos: (f64, f64),
    selected: Option<u32>,
    /// When the simulation was last told what to watch.
    watch_told: Instant,
    tool: Tool,
    kit: Kit,
    press: Option<Press>,
    panning: bool,
    last_mouse: (f32, f32),
    /// Fraction of a sprayed body carried over to the next frame.
    spray_carry: f32,
    seed: u64,
    density: Density,
    meter: fmt::Meter,
    flashes: Vec<(Merge, f64, Instant)>,
    /// Simulated seconds per real second last asked for (kept while paused).
    last_pace: f64,
    menu_open: bool,
    ui_has_pointer: bool,
    ui_has_keyboard: bool,
    settings_dirty: bool,
    /// Name typed for saving, and the worlds found on disk.
    save_name: String,
    saves: Vec<String>,
    status: Option<(String, f64)>,
}

/// Asks the simulation to stand aside for as long as a `Turn` is held: a small world is
/// stepped so often that its locks are otherwise almost never free.
struct Reading(Arc<AtomicBool>);

struct Turn<'a>(&'a AtomicBool);

impl Reading {
    fn begin(&self) -> Turn<'_> {
        self.0.store(true, Ordering::Release);
        Turn(&self.0)
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn worlds_dir() -> Option<PathBuf> {
    Settings::directory().map(|d| d.join("worlds"))
}

fn list_saves() -> Vec<String> {
    let Some(dir) = worlds_dir() else { return Vec::new() };
    let mut names: Vec<String> =
        std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.path().file_name()?.to_str()?.strip_suffix(".gsw").map(str::to_string)).collect();
    names.sort();
    names
}

/// A state on a circular orbit through `(x, y)` around `centre`, counter-clockwise.
fn circling(centre: &Picked, x: f64, y: f64) -> (f64, f64) {
    let (rx, ry) = (x - centre.x, y - centre.y);
    let d = (rx * rx + ry * ry).sqrt().max(1.0);
    let v = (G * centre.mass / d).sqrt();
    (centre.vx - v * ry / d, centre.vy + v * rx / d)
}

/// What pulls hardest at a point, among the heaviest few bodies and the selected one.
fn anchor(heavy: &[Heavy], selected: Option<Picked>, x: f64, y: f64) -> Option<Picked> {
    let listed = heavy.iter().map(|h| Picked { id: h.id, x: h.x, y: h.y, vx: h.vx, vy: h.vy, mass: h.mass as f64, radius: h.radius as f64 });
    listed.chain(selected).max_by(|a, b| {
        let pull = |p: &Picked| p.mass / ((p.x - x).powi(2) + (p.y - y).powi(2)).max(1.0);
        pull(a).total_cmp(&pull(b))
    })
}

/// Where something released at a point with a velocity goes over the next `span` seconds,
/// judged by the pull of the heaviest bodies alone.
fn preview(heavy: &[Heavy], from: (f64, f64), velocity: (f64, f64), span: f64) -> Vec<(f64, f64)> {
    let steps = 240;
    let h = span / steps as f64;
    let (mut x, mut y, mut vx, mut vy) = (from.0, from.1, velocity.0, velocity.1);
    let pull = |x: f64, y: f64, t: f64| {
        heavy.iter().fold((0.0, 0.0), |a, b| {
            let (dx, dy) = (b.x + b.vx * t - x, b.y + b.vy * t - y);
            let d2 = dx * dx + dy * dy + (b.radius as f64).powi(2);
            let f = G * b.mass as f64 / (d2 * d2.sqrt());
            (a.0 + dx * f, a.1 + dy * f)
        })
    };
    let mut path = vec![(x, y)];
    let mut a = pull(x, y, 0.0);
    for k in 1..=steps {
        vx += 0.5 * h * a.0;
        vy += 0.5 * h * a.1;
        x += h * vx;
        y += h * vy;
        a = pull(x, y, k as f64 * h);
        vx += 0.5 * h * a.0;
        vy += 0.5 * h * a.1;
        path.push((x, y));
    }
    path
}

impl Sandbox {
    pub fn start(setup: Setup, settings: &Settings) -> Self {
        let scenario = setup.name.clone();
        let pace = setup.time_scale;
        let sim = gsim_swarm::Sim::new(setup);
        let mut density = Density::new();
        density.set_gpu(settings.gpu);
        Self {
            runner: Runner::start(sim, pace, crate::density::simulation_threads(settings.gpu)),
            scenario,
            view: View { cx: 0.0, cy: 0.0, mpp: 3.0e8 },
            view_ready: false,
            zoom_pending: 0.0,
            offset: (0.0, 0.0),
            last_target: Target::Free,
            last_target_pos: (0.0, 0.0),
            selected: None,
            watch_told: Instant::now(),
            tool: Tool::Look,
            kit: Kit { mass: 1.0e25, density: ROCK, brush: 40.0, rate: 200.0, structure: Structure::Galaxy, count: 5000.0, bulk: 1.0e29, pieces: 60.0, violence: 1.5 },
            press: None,
            panning: false,
            last_mouse: mouse_position(),
            spray_carry: 0.0,
            seed: 1,
            density,
            meter: fmt::Meter::default(),
            flashes: Vec::new(),
            last_pace: pace,
            menu_open: false,
            ui_has_pointer: false,
            ui_has_keyboard: false,
            settings_dirty: false,
            save_name: "world".into(),
            saves: list_saves(),
            status: None,
        }
    }

    fn say(&mut self, text: &str) {
        self.status = Some((text.to_string(), get_time() + 2.5));
    }

    fn select(&mut self, id: Option<u32>) {
        self.selected = id;
        self.watch_told = Instant::now();
        self.runner.send(Command::Watch(id.into_iter().collect()));
    }

    /// Set the pace in simulated seconds per real second (0 pauses).
    fn set_pace(&mut self, pace: f64) {
        if pace > 0.0 {
            self.last_pace = pace;
        }
        self.runner.send(Command::Pace(pace));
    }

    /// Move the picture to the graphics card or back, and hand the simulation the cores that
    /// frees or takes.
    fn use_gpu(&mut self, settings: &mut Settings, on: bool) {
        self.density.set_gpu(on);
        self.runner.send(Command::Threads(crate::density::simulation_threads(on)));
        if settings.gpu != on {
            settings.gpu = on;
            // What the machine holds depends on how many cores simulate.
            settings.measured.clear();
            settings.save();
        }
    }

    fn next_seed(&mut self) -> u64 {
        self.seed = self.seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.seed
    }

    /// Uniform in `[0, 1)`.
    fn random(&mut self) -> f64 {
        (self.next_seed() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn frame(&mut self, settings: &mut Settings) -> Outcome {
        // To notice what this frame changes, by key or in the menu, and save it.
        let before = settings.clone();
        let dt = get_frame_time().min(0.1);
        let hud = settings.ui_factor();
        let ui = settings.marker_factor();
        let keys = !self.ui_has_keyboard && !self.menu_open;
        let mouse = mouse_position();

        let reading = Reading(self.runner.reading.clone());
        let (seen, merges, notice) = {
            let _turn = reading.begin();
            let mut p = self.runner.published.lock().unwrap();
            let seen = Seen { clock: p.clock, time: p.time, pace: p.pace, theta: p.theta, stats: p.stats.clone(), heaviest: p.heaviest.clone(), watch: p.watch.clone() };
            (seen, std::mem::take(&mut p.merges), p.notice.take())
        };
        let now = Instant::now();
        self.flashes.extend(merges.into_iter().map(|(m, t)| (m, t, now)));
        if let Some(text) = notice {
            // A world was saved or swapped: the list on disk, or what is selected, may be stale.
            self.saves = list_saves();
            if text.starts_with("Loaded") || text.starts_with("Went back") {
                self.select(None);
                self.density.clear_trail();
            }
            self.say(&text);
        }

        // --- keys ----------------------------------------------------------------------------
        if is_key_pressed(KeyCode::Escape) {
            // With the button down, Escape calls off what was being placed.
            if self.press.is_some() && self.tool != Tool::Look {
                self.press = None;
            } else {
                self.menu_open = !self.menu_open;
            }
        }
        if is_key_pressed(KeyCode::F3) {
            settings.details = !settings.details;
        }
        if is_key_pressed(KeyCode::F11) {
            settings.fullscreen = !settings.fullscreen;
            set_fullscreen(settings.fullscreen);
        }
        let mut pan = (0.0, 0.0);
        if keys {
            if is_key_pressed(KeyCode::Space) {
                self.set_pace(if seen.pace > 0.0 { 0.0 } else { self.last_pace });
            }
            // < and > (comma and period): held, they change the pace smoothly.
            let turn = is_key_down(KeyCode::Period) as i32 - is_key_down(KeyCode::Comma) as i32;
            if turn != 0 {
                self.set_pace(self.last_pace * 2f64.powf(turn as f64 * PACE_KEY_RATE * dt as f64));
            }
            if is_key_pressed(KeyCode::C) {
                let at = ColorMode::ALL.iter().position(|m| *m == settings.color).unwrap_or(0);
                settings.color = ColorMode::ALL[(at + 1) % ColorMode::ALL.len()];
                self.say(&format!("Colour shows {}", settings.color.name()));
            }
            if is_key_pressed(KeyCode::X) {
                settings.long_exposure = !settings.long_exposure;
            }
            if is_key_pressed(KeyCode::F) {
                settings.camera = if settings.camera == Camera::Ship { Camera::Selection } else { Camera::Ship };
            }
            for (tool, _, key) in Tool::ALL {
                if is_key_pressed(key) {
                    self.tool = tool;
                }
            }
            if let (true, Some(id)) = (is_key_pressed(KeyCode::Delete) || is_key_pressed(KeyCode::Backspace), self.selected) {
                self.runner.send(Command::Remove(id));
                self.select(None);
            }
            pan = ((is_key_down(KeyCode::D) as i32 - is_key_down(KeyCode::A) as i32) as f64, (is_key_down(KeyCode::S) as i32 - is_key_down(KeyCode::W) as i32) as f64);
            self.zoom_pending += 25.0 * dt * (is_key_down(KeyCode::E) as i32 - is_key_down(KeyCode::Q) as i32) as f32;
        }
        self.density.long_exposure = settings.long_exposure;
        // A few thousand bodies are each drawn to be seen; a crowd is drawn as a crowd.
        self.density.sparse = seen.stats.bodies <= 4000;

        // A watched body that merged lives on in its survivor.
        if self.watch_told.elapsed().as_secs_f32() > 0.3 && self.selected.is_some() && seen.watch.first().is_some_and(|w| Some(*w) != self.selected) {
            self.selected = seen.watch.first().copied();
        }

        clear_background(style::BACKGROUND);
        let pointer = !self.ui_has_pointer && !self.menu_open;
        let wheel = mouse_wheel().1;
        let reach = (PICK_RADIUS_PX * ui) as f64;
        let brush_px = self.kit.brush * ui;
        let shift = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
        let left_pressed = pointer && is_mouse_button_pressed(MouseButton::Left);
        let left_released = is_mouse_button_released(MouseButton::Left);
        let (big, present, picked, hovered, bodies_n, frame_v, target_pos);
        let mut clicked: Option<Option<u32>> = None;
        let mut moved_view = false;
        {
            let _turn = reading.begin();
            let b = self.runner.bodies.read().unwrap();
            bodies_n = b.len();
            // Between two steps the bodies are drawn part of the way along the last one.
            present = seen.clock.time + seen.clock.at.elapsed().as_secs_f64() * seen.clock.rate;
            let tau = (present - b.time).clamp(-b.dt, 0.0);
            let at = |i: usize| Picked { id: b.id[i], x: b.x[i] + b.vx[i] * tau, y: b.y[i] + b.vy[i] * tau, vx: b.vx[i], vy: b.vy[i], mass: b.m[i] as f64, radius: b.r[i] as f64 };
            let chosen = self.selected.and_then(|id| b.locate(id));
            if chosen.is_none() {
                self.selected = None;
            }
            picked = chosen.map(at);

            // --- camera ------------------------------------------------------------------------
            let target = match (settings.camera != Camera::Ship, picked) {
                (true, Some(p)) => Target::Body(p.id),
                _ => Target::Free,
            };
            target_pos = match (target, picked) {
                (Target::Body(_), Some(p)) => (p.x, p.y),
                _ => self.last_target_pos,
            };
            frame_v = match (target, picked) {
                (Target::Body(_), Some(p)) => (p.vx, p.vy),
                _ => (0.0, 0.0),
            };
            if !self.view_ready && b.is_empty() {
                // Nothing to frame: start at a scale where the first few bodies make sense.
                self.view_ready = true;
            } else if !self.view_ready {
                // Start with the whole world in view.
                let span = |v: &Vec<f64>| v.par_iter().fold(|| (f64::MAX, f64::MIN), |a, v| (a.0.min(*v), a.1.max(*v))).reduce(|| (f64::MAX, f64::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
                let ((x0, x1), (y0, y1)) = (span(&b.x), span(&b.y));
                self.offset = (0.5 * (x0 + x1) - target_pos.0, 0.5 * (y0 + y1) - target_pos.1);
                self.view.mpp = (((x1 - x0) / screen_width() as f64).max((y1 - y0) / screen_height() as f64) * 1.15).max(1.0e4);
                self.view_ready = true;
            } else if target != self.last_target {
                // Keep the picture still when the thing being followed changes.
                self.offset = (self.view.cx - target_pos.0, self.view.cy - target_pos.1);
                moved_view = true;
            }
            self.last_target = target;
            self.last_target_pos = target_pos;
            if pointer && wheel != 0.0 {
                self.zoom_pending += (wheel * settings.zoom_speed).clamp(-1.5, 1.5);
            }
            if self.zoom_pending.abs() > 1e-3 {
                let step = self.zoom_pending * (12.0 * dt).min(1.0);
                self.zoom_pending -= step;
                // Zoom about the cursor: the point under it stays put.
                self.view.cx = target_pos.0 + self.offset.0;
                self.view.cy = target_pos.1 + self.offset.1;
                let before = self.view.to_world(mouse.0, mouse.1);
                self.view.mpp = (self.view.mpp * 1.1f64.powf(-step as f64)).clamp(0.05, 1.0e13);
                let after = self.view.to_world(mouse.0, mouse.1);
                self.offset.0 += before.0 - after.0;
                self.offset.1 += before.1 - after.1;
                moved_view = true;
            }
            if pan != (0.0, 0.0) {
                let stride = PAN_RATE * dt as f64 * screen_height() as f64 * self.view.mpp;
                self.offset = (self.offset.0 + pan.0 * stride, self.offset.1 + pan.1 * stride);
                moved_view = true;
            }
            // The right and middle buttons always pan; the left one does with the Look tool.
            if pointer && (is_mouse_button_pressed(MouseButton::Right) || is_mouse_button_pressed(MouseButton::Middle)) {
                self.panning = true;
            }
            self.panning &= is_mouse_button_down(MouseButton::Right) || is_mouse_button_down(MouseButton::Middle);
            let (dx, dy) = (mouse.0 - self.last_mouse.0, mouse.1 - self.last_mouse.1);
            if let Some(press) = self.press.as_mut() {
                press.moved += dx.abs() + dy.abs();
            }
            let dragging = self.panning || (self.tool == Tool::Look && self.press.as_ref().is_some_and(|p| p.moved > 10.0 * ui));
            if dragging && (dx, dy) != (0.0, 0.0) {
                self.offset.0 -= dx as f64 * self.view.mpp;
                self.offset.1 -= dy as f64 * self.view.mpp;
                moved_view = true;
            }
            self.last_mouse = mouse;
            self.view.cx = target_pos.0 + self.offset.0;
            self.view.cy = target_pos.1 + self.offset.1;
            let view = self.view;
            let (mx, my) = view.to_world(mouse.0, mouse.1);

            // --- what is under the cursor ------------------------------------------------------
            let steady = self.press.as_ref().is_none_or(|p| p.moved <= 10.0 * ui);
            let under = (pointer && steady && matches!(self.tool, Tool::Look | Tool::Shatter))
                .then(|| {
                    // The heaviest body right under the cursor, else the nearest one around it.
                    let within = reach * view.mpp;
                    let close = 0.4 * within;
                    (0..b.len())
                        .into_par_iter()
                        .filter_map(|i| {
                            let d = ((b.x[i] + b.vx[i] * tau - mx).powi(2) + (b.y[i] + b.vy[i] * tau - my).powi(2)).sqrt() - b.r[i] as f64;
                            (b.m[i] > 0.0 && d <= within).then_some((i, d, b.m[i]))
                        })
                        .reduce_with(|p, q| match (p.1 <= close, q.1 <= close) {
                            (true, true) if q.2 > p.2 => q,
                            (false, true) => q,
                            (false, false) if q.1 < p.1 => q,
                            _ => p,
                        })
                        .map(|c| at(c.0))
                })
                .flatten();
            hovered = under.filter(|h| Some(h.id) != self.selected);
            if left_released && self.press.is_some() && steady && under.is_some() | (self.tool == Tool::Look) {
                clicked = Some(under.map(|p| p.id));
            }
            if left_pressed {
                self.press = Some(Press { at: (mx - target_pos.0, my - target_pos.1), moved: 0.0 });
            }

            if moved_view {
                // Light that lingered belongs to the old view.
                self.density.clear_trail();
            }
            big = self.density.project(&b, &view, tau, settings.color);
        }
        let view = self.view;
        let (mx, my) = view.to_world(mouse.0, mouse.1);
        let pace = if seen.pace > 0.0 { seen.pace } else { self.last_pace };

        // --- the world -------------------------------------------------------------------------
        self.flashes.retain(|f| f.2.elapsed().as_secs_f32() < FLASH_SECONDS);
        let flashes: Vec<Flash> = self
            .flashes
            .iter()
            .map(|(m, t, since)| {
                let moved = present - t;
                Flash { x: m.x + m.vx * moved, y: m.y + m.vy * moved, life: (1.0 - since.elapsed().as_secs_f32() / FLASH_SECONDS).clamp(0.0, 1.0), mass: m.mass }
            })
            .collect();
        self.density.present(&flashes);
        for b in &big {
            style::body((b.x, b.y), b.r, b.mass as f64, b.id, b.fade, ui);
        }
        let mut labels = style::Labels::default();

        // --- tools -----------------------------------------------------------------------------
        let arrow = |from: (f32, f32), to: (f32, f32), color: Color| {
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 3.0 || !len.is_finite() {
                return;
            }
            draw_line(from.0, from.1, to.0, to.1, 1.5 * ui, color);
            let (ux, uy) = (dx / len, dy / len);
            let head = (7.0 * ui).min(0.5 * len);
            let tip = |side: f32| vec2(to.0 - ux * head - uy * side * 0.5 * head, to.1 - uy * head + ux * side * 0.5 * head);
            draw_triangle(vec2(to.0, to.1), tip(1.0), tip(-1.0), color);
        };
        match (self.tool, pointer) {
            (Tool::Spray | Tool::Erase, true) => {
                let colour = if self.tool == Tool::Erase { style::EMBER } else { style::ACCENT };
                style::ring(mouse.0, mouse.1, brush_px, 1.5 * ui, style::alpha(colour, 0.8));
            }
            (Tool::Drop, true) if self.press.is_none() => style::ring(mouse.0, mouse.1, brush_px, 1.5 * ui, style::alpha(style::GOLD, 0.6)),
            _ => {}
        }
        // A new body's velocity: the drag is where it will be a second from now, in the frame
        // of whatever the camera follows; with Shift, a circular orbit around what pulls hardest.
        let throw = |from: (f64, f64)| -> (f64, f64) {
            match anchor(&seen.heaviest, picked, from.0, from.1).filter(|_| shift) {
                Some(centre) => circling(&centre, from.0, from.1),
                None => (frame_v.0 + (mx - from.0) / pace, frame_v.1 + (my - from.1) / pace),
            }
        };
        if let (Tool::Place | Tool::Drop, Some(press)) = (self.tool, &self.press) {
            let from = (target_pos.0 + press.at.0, target_pos.1 + press.at.1);
            let v = throw(from);
            let s = view.to_screen(from.0, from.1);
            let mut prev: Option<(f32, f32)> = None;
            let path = preview(&seen.heaviest, from, v, PREVIEW_SECONDS * pace);
            for (k, q) in path.iter().enumerate() {
                // Drawn in the followed body's frame, like everything else.
                let t = k as f64 / (path.len() - 1) as f64 * PREVIEW_SECONDS * pace;
                let p = view.to_screen(q.0 - frame_v.0 * t, q.1 - frame_v.1 * t);
                if let Some(a) = prev.filter(|a| view.on_screen(*a, 200.0) || view.on_screen(p, 200.0)) {
                    draw_line(a.0, a.1, p.0, p.1, 1.5 * ui, Color::from_rgba(90, 255, 120, 150));
                }
                prev = Some(p);
            }
            if self.tool == Tool::Drop {
                style::ring(s.0, s.1, brush_px, 1.5 * ui, style::alpha(style::GOLD, 0.9));
            } else {
                style::disc(s.0, s.1, ((radius_from_mass(self.kit.mass, self.kit.density) / view.mpp) as f32).max(2.5 * ui), style::mass_color(self.kit.mass));
            }
            // The arrow is the velocity as seen in the picture: where it is a second on.
            let ahead = view.to_screen(from.0 + (v.0 - frame_v.0) * pace, from.1 + (v.1 - frame_v.1) * pace);
            arrow(s, ahead, WHITE);
            let speed = ((v.0 - frame_v.0).powi(2) + (v.1 - frame_v.1).powi(2)).sqrt();
            labels.push(fmt::speed(speed), ahead.0, ahead.1 - 10.0 * ui, LABEL * ui, WHITE, Rank::Selection);
        }
        // What the tools do to the world.
        let mut commands: Vec<Command> = Vec::new();
        if left_released {
            if let Some(press) = self.press.take() {
                let from = (target_pos.0 + press.at.0, target_pos.1 + press.at.1);
                let v = throw(from);
                match (self.tool, self.kit.structure == Structure::Ring, picked) {
                    (Tool::Place, _, _) => {
                        let mut new = Bodies::default();
                        new.push(from.0, from.1, v.0, v.1, self.kit.mass, radius_from_mass(self.kit.mass, self.kit.density), 3);
                        commands.push(Command::Add(Box::new(new)));
                    }
                    (Tool::Drop, true, None) => self.say("Select the body the ring should circle first"),
                    (Tool::Drop, ring, body) => {
                        // A ring goes around the selected body, wherever the press was.
                        let (centre, at, velocity) = match body.filter(|_| ring) {
                            Some(p) => (p.mass, (p.x, p.y), (p.vx, p.vy)),
                            None => (0.0, from, v),
                        };
                        let seed = self.next_seed();
                        let mut new = scenario::structure(self.kit.structure, seed, self.kit.count as usize, brush_px as f64 * view.mpp, self.kit.bulk, centre, 3);
                        for i in 0..new.len() {
                            new.r[i] = radius_from_mass(new.m[i] as f64, self.kit.density) as f32;
                        }
                        for i in 0..new.len() {
                            new.x[i] += at.0;
                            new.y[i] += at.1;
                            new.vx[i] += velocity.0;
                            new.vy[i] += velocity.1;
                        }
                        commands.push(Command::Add(Box::new(new)));
                    }
                    _ => {}
                }
            }
        }
        if pointer && is_mouse_button_down(MouseButton::Left) && self.press.is_some() {
            match self.tool {
                Tool::Spray => {
                    self.spray_carry += self.kit.rate * dt;
                    let count = self.spray_carry as usize;
                    self.spray_carry -= count as f32;
                    let mut new = Bodies::default();
                    for _ in 0..count {
                        let (r, a) = (brush_px as f64 * view.mpp * self.random().sqrt(), self.random() * std::f64::consts::TAU);
                        let at = (mx + r * a.cos(), my + r * a.sin());
                        // Moving with the picture, or with Shift each on its own orbit.
                        let v = anchor(&seen.heaviest, picked, at.0, at.1).filter(|_| shift).map_or(frame_v, |c| circling(&c, at.0, at.1));
                        new.push(at.0, at.1, v.0, v.1, self.kit.mass, radius_from_mass(self.kit.mass, self.kit.density), 3);
                    }
                    if !new.is_empty() {
                        commands.push(Command::Add(Box::new(new)));
                    }
                }
                Tool::Erase => commands.push(Command::Erase { x: mx, y: my, r: brush_px as f64 * view.mpp }),
                _ => {}
            }
        }
        if !is_mouse_button_down(MouseButton::Left) {
            self.press = None;
        }
        match (clicked, self.tool) {
            (Some(Some(id)), Tool::Shatter) => commands.push(Command::Shatter { id, pieces: self.kit.pieces as usize, violence: self.kit.violence }),
            (Some(id), Tool::Look) => self.select(id),
            _ => {}
        }

        // --- selection and hover -----------------------------------------------------------------
        let outline = |p: &Picked, color: Color| {
            let s = view.to_screen(p.x, p.y);
            let r_px = ((p.radius / view.mpp) as f32).min(4000.0) + 6.0 * ui;
            draw_rectangle_lines(s.0 - r_px, s.1 - r_px, 2.0 * r_px, 2.0 * r_px, 1.5 * ui, color);
            (s, r_px)
        };
        if let Some(p) = &picked {
            let (s, r_px) = outline(p, WHITE);
            // Where it will be a second from now.
            arrow(s, view.to_screen(p.x + (p.vx - frame_v.0) * pace, p.y + (p.vy - frame_v.1) * pace), style::alpha(WHITE, 0.8));
            let lines = vec![format!("B{}", p.id), format!("m = {}", fmt::mass(p.mass)), format!("r = {}", fmt::distance(p.radius)), format!("v = {}", fmt::speed((p.vx * p.vx + p.vy * p.vy).sqrt()))];
            labels.block(lines, s.0, s.1 + r_px + 12.0 * ui, LABEL * ui, WHITE, Rank::Selection);
        }
        if let Some(h) = &hovered {
            let (s, r_px) = outline(h, style::alpha(style::ACCENT, 0.9));
            arrow(s, view.to_screen(h.x + (h.vx - frame_v.0) * pace, h.y + (h.vy - frame_v.1) * pace), style::alpha(style::ACCENT, 0.8));
            let mut lines = vec![format!("B{}", h.id), format!("m = {}", fmt::mass(h.mass))];
            match &picked {
                // Measured from the selected body.
                Some(p) => {
                    lines.push(format!("d = {}", fmt::distance(((h.x - p.x).powi(2) + (h.y - p.y).powi(2)).sqrt())));
                    lines.push(format!("rel v = {}", fmt::speed(((h.vx - p.vx).powi(2) + (h.vy - p.vy).powi(2)).sqrt())));
                }
                None => lines.push(format!("v = {}", fmt::speed((h.vx * h.vx + h.vy * h.vy).sqrt()))),
            }
            labels.block(lines, s.0, s.1 + r_px + 12.0 * ui, LABEL * ui, style::ACCENT, Rank::Approach);
        }
        labels.draw();
        style::scale_bar(screen_width() * 0.5, screen_height() - 18.0 * hud, view.mpp, fmt::distance_round, hud);

        let outcome = self.hud(settings, &before, &seen, bodies_n, picked, hud, &mut commands);
        for command in commands {
            self.runner.send(command);
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn hud(&mut self, settings: &mut Settings, before: &Settings, seen: &Seen, bodies: usize, picked: Option<Picked>, hud: f32, commands: &mut Vec<Command>) -> Outcome {
        self.meter.frame(self.density.last_ms);
        let stats = &seen.stats;
        let title = format!("{}   ·   {} BODIES", self.scenario.to_uppercase(), group_digits(bodies));
        // What one real second is worth, as achieved; and as asked for when that is more.
        let pace_line = match seen.pace {
            p if p <= 0.0 => "PAUSED".to_string(),
            p if stats.achieved > 0.0 && stats.achieved < 0.9 * p => format!("1 s = {}  (of {})", fmt::span(stats.achieved), fmt::span(p)),
            p => format!("1 s = {}", fmt::span(p)),
        };
        let stat_lines = [
            format!("{} bodies   {:.0} fps   draw {:.1} ms", group_digits(bodies), self.meter.fps, self.meter.ms),
            format!("{} steps per second, {} each", group_digits(stats.steps_per_s as usize), fmt::span(stats.dt)),
            format!("step {:.2} ms  (sort {:.1}  gravity {:.1}  merge {:.1})", stats.step_ms, stats.sort_ms, stats.force_ms, stats.finish_ms),
            format!("{}   {:.0} pulls per body   {}", stats.mode, stats.interactions, stats.level),
            if stats.limited { "steps held short for a tight orbit".to_string() } else { "steps as long as the pace asks".to_string() },
            match stats.error {
                Some(e) => format!("force error {:.3} %   angle {:.2}", e * 100.0, seen.theta),
                None => "force error: measuring".to_string(),
            },
            format!("day {:.0}   merges {} ({:.0}/s)   escaped {}", seen.time / 86_400.0, group_digits(stats.merges_total as usize), stats.merges_per_s, stats.removed_total),
        ];
        let status = self.status.as_ref().filter(|s| s.1 > get_time()).map(|s| s.0.clone());
        let (speed, theta) = (seen.pace, seen.theta);
        let mut menu = self.menu_open;
        let (mut has_ptr, mut has_kb) = (false, false);
        let mut options = settings.clone();
        let (mut new_speed, mut new_theta, mut new_pick) = (None, theta, None);
        let mut outcome = Outcome::Continue;
        let mut tool = self.tool;
        let kit = &mut self.kit;
        let (save_name, saves) = (&mut self.save_name, &self.saves);
        let selected = self.selected;
        egui_macroquad::ui(|ctx| {
            use egui::{Align2, Area, Id, RichText};
            ctx.set_zoom_factor(hud);
            let (gold, dim) = (style::c32(style::GOLD), style::c32(style::DIM));

            Area::new(Id::new("title")).anchor(Align2::CENTER_TOP, [0.0, 10.0]).interactable(false).show(ctx, |ui| {
                // One line each, however long: wrapped, the title looks broken.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(&title).small().color(dim).extra_letter_spacing(2.0));
                    ui.label(RichText::new(&pace_line).heading().color(if speed > 0.0 { style::c32(style::TEXT) } else { gold }));
                });
            });
            // Under the title: time buttons, then whatever was just announced.
            Area::new(Id::new("time")).anchor(Align2::CENTER_TOP, [0.0, 62.0]).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (value, label) in PACES {
                        let on = (value - speed).abs() <= 0.02 * value;
                        let text = RichText::new(label).monospace().color(if on { gold } else { style::c32(style::TEXT) });
                        if ui.add(egui::Button::new(text).frame(on)).clicked() {
                            new_speed = Some(value);
                        }
                    }
                });
            });
            if let Some(s) = &status {
                Area::new(Id::new("status")).anchor(Align2::CENTER_TOP, [0.0, 90.0]).interactable(false).show(ctx, |ui| {
                    ui.label(RichText::new(s).color(gold));
                });
            }

            if options.details {
                Area::new(Id::new("stats")).anchor(Align2::LEFT_TOP, [10.0, 10.0]).interactable(false).show(ctx, |ui| {
                    style::panel().show(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        for l in &stat_lines {
                            ui.label(RichText::new(l).monospace().color(dim));
                        }
                    });
                });
            }

            // Top right: the heavyweights. Click one to select it.
            Area::new(Id::new("heavy")).anchor(Align2::RIGHT_TOP, [-10.0, 10.0]).show(ctx, |ui| {
                style::panel().show(ui, |ui| {
                    style::caption(ui, "HEAVIEST");
                    egui::Grid::new("heaviest").num_columns(3).spacing([12.0, 2.0]).show(ui, |ui| {
                        for h in seen.heaviest.iter().take(8) {
                            let colour = if selected == Some(h.id) { gold } else { style::c32(style::mass_color(h.mass as f64)) };
                            if ui.add(egui::Label::new(RichText::new(format!("B{}", h.id)).color(colour)).sense(egui::Sense::click())).clicked() {
                                new_pick = Some(h.id);
                            }
                            ui.label(RichText::new(fmt::mass(h.mass as f64)).color(dim));
                            ui.label(RichText::new(fmt::distance(h.radius as f64)).color(dim));
                            ui.end_row();
                        }
                    });
                });
            });

            // Bottom left: the tools, and what the chosen one is set to.
            Area::new(Id::new("tools")).anchor(Align2::LEFT_BOTTOM, [10.0, -10.0]).show(ctx, |ui| {
                style::panel().show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.spacing_mut().slider_width = 220.0;
                    // The ranges are only what the sliders cover: anything may be typed.
                    let free = egui::SliderClamping::Never;
                    let mass = |ui: &mut egui::Ui, value: &mut f64, label: &str| {
                        ui.add(egui::Slider::new(value, 1.0e18..=1.0e31).logarithmic(true).clamping(free).custom_formatter(|v, _| fmt::mass(v)).text(label));
                    };
                    let hint = |ui: &mut egui::Ui, text: &str| {
                        ui.label(RichText::new(text).small().color(dim));
                    };
                    let density = |ui: &mut egui::Ui, value: &mut f64| {
                        ui.add(egui::Slider::new(value, 100.0..=1.0e6).logarithmic(true).clamping(free).max_decimals(0).suffix(" kg/m³").text("density")).on_hover_text(
                            "What new bodies are made of, which sets how large they are. Water is 1,000, rock about 3,000, the Earth 5,514, the Sun 1,408, iron 7,900. Denser bodies are smaller and so collide less often.",
                        );
                    };
                    match tool {
                        Tool::Look => hint(ui, "Click a body to select it; drag to move the view."),
                        Tool::Place => {
                            mass(ui, &mut kit.mass, "mass");
                            density(ui, &mut kit.density);
                            hint(ui, "Click to place, drag to throw. Shift: on a circular orbit.");
                        }
                        Tool::Spray => {
                            mass(ui, &mut kit.mass, "mass of each");
                            density(ui, &mut kit.density);
                            ui.add(egui::Slider::new(&mut kit.rate, 5.0..=5000.0).logarithmic(true).clamping(free).text("bodies a second"));
                            ui.add(egui::Slider::new(&mut kit.brush, 5.0..=300.0).clamping(free).text("brush"));
                            hint(ui, "Hold to spray. Shift: each on a circular orbit.");
                        }
                        Tool::Drop => {
                            ui.horizontal(|ui| {
                                for kind in Structure::ALL {
                                    ui.selectable_value(&mut kit.structure, kind, kind.name());
                                }
                            });
                            ui.add(egui::Slider::new(&mut kit.count, 100.0..=200_000.0).logarithmic(true).clamping(free).integer().text("bodies"));
                            mass(ui, &mut kit.bulk, "mass in all");
                            density(ui, &mut kit.density);
                            ui.add(egui::Slider::new(&mut kit.brush, 5.0..=300.0).clamping(free).text("size"));
                            hint(ui, if kit.structure == Structure::Ring { "Click anywhere: it goes around the selected body." } else { "Click to drop, drag to throw. Shift: on a circular orbit." });
                        }
                        Tool::Erase => {
                            ui.add(egui::Slider::new(&mut kit.brush, 5.0..=300.0).clamping(free).text("brush"));
                            hint(ui, "Hold to remove everything under the brush.");
                        }
                        Tool::Shatter => {
                            ui.add(egui::Slider::new(&mut kit.pieces, 2.0..=1000.0).logarithmic(true).clamping(free).integer().text("pieces"));
                            ui.add(egui::Slider::new(&mut kit.violence, 0.3..=5.0).clamping(free).text("violence"));
                            hint(ui, "Click a body to break it apart.");
                        }
                    }
                    // Whatever was typed, the tools are handed something they can use.
                    let positive = |v: f64, least: f64, most: f64| if v.is_finite() { v.clamp(least, most) } else { least };
                    kit.mass = positive(kit.mass, 1.0, 1.0e36);
                    kit.bulk = positive(kit.bulk, 1.0, 1.0e36);
                    kit.density = positive(kit.density, 1.0e-3, 1.0e18);
                    kit.count = positive(kit.count, 1.0, 2.0e6).round();
                    kit.pieces = positive(kit.pieces, 2.0, 1.0e5).round();
                    kit.violence = positive(kit.violence, 0.0, 1.0e3);
                    kit.rate = positive(kit.rate as f64, 0.1, 1.0e5) as f32;
                    kit.brush = positive(kit.brush as f64, 1.0, 1.0e4) as f32;
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        for (k, (which, name, _)) in Tool::ALL.into_iter().enumerate() {
                            ui.selectable_value(&mut tool, which, format!("{} {name}", k + 1));
                        }
                    });
                });
            });

            // Bottom right: the selected body, open to change.
            if let Some(p) = picked {
                Area::new(Id::new("body")).anchor(Align2::RIGHT_BOTTOM, [-10.0, -10.0]).show(ctx, |ui| {
                    style::panel().show(ui, |ui| {
                        ui.set_width(250.0);
                        style::caption(ui, &format!("B{}", p.id));
                        let was = ((p.vx * p.vx + p.vy * p.vy).sqrt(), p.vy.atan2(p.vx).to_degrees());
                        let (mut mass, mut radius, mut speed, mut heading) = (p.mass, p.radius, was.0, was.1);
                        // Only what the player moves counts as a change: a slider may round or
                        // clamp what it is shown.
                        let mut changed = ui.add(egui::Slider::new(&mut mass, 1.0e15..=1.0e32).logarithmic(true).custom_formatter(|v, _| fmt::mass(v)).text("mass")).changed();
                        changed |= ui.add(egui::Slider::new(&mut radius, 1.0e3..=1.0e10).logarithmic(true).custom_formatter(|v, _| fmt::distance(v)).text("radius")).changed();
                        changed |= ui.add(egui::Slider::new(&mut speed, 0.0..=1.0e6).logarithmic(true).smallest_positive(1.0).custom_formatter(|v, _| fmt::speed(v)).text("speed")).changed();
                        changed |= ui.add(egui::Slider::new(&mut heading, -180.0..=180.0).suffix("°").text("heading")).changed();
                        ui.horizontal(|ui| {
                            if ui.button("Stop").clicked() {
                                (speed, changed) = (0.0, true);
                            }
                            if ui.button("Delete").clicked() {
                                commands.push(Command::Remove(p.id));
                            }
                        });
                        if changed {
                            let a = heading.to_radians();
                            commands.push(Command::Edit { id: p.id, mass, radius, vx: speed * a.cos(), vy: speed * a.sin() });
                        }
                    });
                });
            }

            if menu {
                egui::Window::new("MENU").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                    ui.set_width(330.0);
                    crate::options::scrolled(ui, |ui| {
                        style::section(ui, "WORLD");
                        ui.add(egui::Slider::new(&mut new_theta, 0.3..=1.2).text("opening angle")).on_hover_text(
                            "How readily distant groups of bodies are treated as one lump when there are too many to sum pair by pair: \
                             smaller is more accurate and slower.",
                        );
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(save_name).desired_width(150.0).hint_text("name"));
                            let name: String = save_name.chars().filter(|c| c.is_alphanumeric() || "-_ ".contains(*c)).collect();
                            if let (true, Some(dir)) = (ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Save")).clicked(), worlds_dir()) {
                                commands.push(Command::Save(dir.join(format!("{}.gsw", name.trim()))));
                            }
                        });
                        for name in saves {
                            ui.horizontal(|ui| {
                                if let (true, Some(dir)) = (ui.small_button("load").clicked(), worlds_dir()) {
                                    commands.push(Command::Load(dir.join(format!("{name}.gsw"))));
                                }
                                ui.label(RichText::new(name).color(dim));
                            });
                        }
                        let back = format!("Back to the last checkpoint ({} kept)", stats.checkpoints);
                        let hover = "The world is remembered every fifteen seconds while it runs; this returns to the latest of those moments.";
                        if ui.add_enabled(stats.checkpoints > 0, egui::Button::new(back)).on_hover_text(hover).clicked() {
                            commands.push(Command::Rewind);
                        }
                        crate::options::show(ui, &mut options, crate::options::World::Large);
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Resume").clicked() {
                            menu = false;
                        }
                        if ui.button("Leave").clicked() {
                            outcome = Outcome::ToMenu(String::new());
                        }
                        if ui.button("Quit").clicked() {
                            outcome = Outcome::Quit;
                        }
                    });
                });
            }
            has_ptr = ctx.wants_pointer_input() || ctx.is_pointer_over_area();
            has_kb = ctx.wants_keyboard_input();
        });
        egui_macroquad::draw();

        self.tool = tool;
        if commands.iter().any(|c| matches!(c, Command::Remove(_))) {
            self.select(None);
        }
        if let Some(id) = new_pick {
            self.select(Some(id));
        }
        if let Some(pace) = new_speed {
            self.set_pace(pace);
        }
        if new_theta != theta {
            self.runner.send(Command::Theta(new_theta));
        }
        if options.gpu != settings.gpu {
            self.use_gpu(settings, options.gpu);
            options.measured.clear();
        }
        if let Some(why) = self.density.take_notice() {
            self.say(&format!("Unable to draw on the graphics card: {why}"));
            self.use_gpu(settings, false);
            options.gpu = false;
        }
        if options != *before {
            *settings = options;
            self.settings_dirty = true;
        } else if self.settings_dirty && !is_mouse_button_down(MouseButton::Left) {
            self.settings_dirty = false;
            settings.save();
        }
        self.menu_open = menu;
        self.ui_has_pointer = has_ptr;
        self.ui_has_keyboard = has_kb;
        outcome
    }
}

/// `1234567` as `1,234,567`.
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
