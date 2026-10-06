//! In-game screen: input, camera, rendering and HUD. All game logic lives in
//! `gsim-client-core`; this file only looks at the replica and draws it.

use crate::chat::{self, ChatBox, Mention};
use crate::fmt;
use crate::style::{self, Rank};
use crate::predictor::{Job, Predictor};
use crate::settings::Settings;
use egui_macroquad::egui;
use gsim_client_core::complete::Context;
use gsim_client_core::net::NetClient;
use gsim_server::scenario::PRESETS;
use gsim_client_core::world::World;
use gsim_client_core::{Controls, EffectKind, SessionConfig};
use gsim_core::objective::orbit_status;
use gsim_core::{math, EphRow, Particle, SystemFrame, Tick};
use gsim_proto::PlayerId;
use gsim_server::net::{build_authority, run, ServerOptions};
use macroquad::prelude::*;
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// In-process server for solo play.
pub struct Solo {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Solo {
    pub fn start(preset: &str, seed: u64) -> Result<(Self, SocketAddr), String> {
        // Let the OS pick a free port, then hand it to the server.
        let port = UdpSocket::bind("127.0.0.1:0").and_then(|s| s.local_addr()).map_err(|e| e.to_string())?.port();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        // Whoever hosts a solo game may use every command.
        let opts = ServerOptions { bind: addr, preset: preset.to_string(), seed, quiet: true, max_clients: 4, op_all: true, ..Default::default() };
        build_authority(&opts)?; // surface configuration errors here rather than in the thread
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("gsim-solo-server".into())
            .spawn(move || {
                if let Err(e) = run(opts, flag) {
                    eprintln!("solo server: {e}");
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((Self { stop, thread: Some(thread) }, addr))
    }
}

impl Drop for Solo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Camera: world metres (f64) to screen pixels, relative to a double-precision centre so
/// nothing loses precision at astronomical distances.
#[derive(Clone, Copy)]
struct View {
    cx: f64,
    cy: f64,
    /// Metres per pixel.
    mpp: f64,
}

impl View {
    fn to_screen(&self, x: f64, y: f64) -> (f32, f32) {
        (
            ((x - self.cx) / self.mpp) as f32 + screen_width() * 0.5,
            ((y - self.cy) / self.mpp) as f32 + screen_height() * 0.5,
        )
    }

    fn to_world(&self, sx: f32, sy: f32) -> (f64, f64) {
        (
            self.cx + (sx - screen_width() * 0.5) as f64 * self.mpp,
            self.cy + (sy - screen_height() * 0.5) as f64 * self.mpp,
        )
    }

    fn on_screen(&self, s: (f32, f32), margin: f32) -> bool {
        s.0 > -margin && s.1 > -margin && s.0 < screen_width() + margin && s.1 < screen_height() + margin
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Ship,
    /// Another player'"'"'s ship (picked by clicking their name in chat).
    Player(PlayerId),
    Body(u32),
    Free,
}

/// A sampled past state, kept for drawing trails relative to any reference body.
struct TrailFrame {
    row: Arc<EphRow>,
    ships: Vec<(PlayerId, f64, f64)>,
}


pub enum Outcome {
    Continue,
    ToMenu(String),
    Quit,
}

pub struct Game {
    net: NetClient,
    _solo: Option<Solo>,
    view: View,
    settings_dirty: bool,
    view_ready: bool,
    zoomed_for_ship: bool,
    had_ship: bool,
    zoom_pending: f32,
    /// Camera offset from whatever it follows.
    offset: (f64, f64),
    follow_selection: bool,
    /// Set when our ship dies while the camera is on a body: the camera stays with the ship
    /// until the player picks a body or toggles follow again.
    follow_paused: bool,
    /// Show body names: on the map, and as the first line of the selected body's label.
    show_names: bool,
    chat: ChatBox,
    /// Camera follows this player (set by clicking their name in chat).
    watch: Option<PlayerId>,
    last_target: Target,
    last_target_pos: (f64, f64),
    selected: Option<u32>,
    heading: f64,
    thrust_pct: f32,
    mouse_aim: bool,
    show_trails: bool,
    trails_relative: bool,
    show_prediction: bool,
    show_shell_prediction: bool,
    show_net: bool,
    menu_open: bool,
    ui_has_pointer: bool,
    ui_has_keyboard: bool,
    drag_from: Option<(f32, f32)>,
    drag_moved: f32,
    last_mouse: (f32, f32),
    frames: VecDeque<TrailFrame>,
    last_frame_tick: Tick,
    predictor: Predictor,
    lookahead_ticks: u32,
    status: Option<(String, f64)>,
    started: f64,
}

const TRAIL_EVERY_TICKS: Tick = 4;
const TRAIL_FRAMES: usize = 150;
const PICK_RADIUS_PX: f32 = 26.0;
const TURN_RATE: f64 = 2.85;
/// Font size of labels drawn in the world, before marker scaling.
const LABEL: f32 = 12.0;

impl Game {
    pub fn connect(addr: SocketAddr, settings: &Settings, solo: Option<Solo>, lookahead_seconds: f32, show_net: bool) -> Result<Self, String> {
        let cfg = SessionConfig {
            name: settings.name.clone(),
            identity: settings.identity(),
            threaded_eph: true,
            // Tick rate is only known after joining; 60 Hz is the server default.
            lookahead_ticks: (lookahead_seconds.clamp(2.0, 60.0) * 60.0) as u32,
            inline_budget: 0,
        };
        Ok(Self {
            net: NetClient::connect(addr, cfg)?,
            _solo: solo,
            view: View { cx: 0.0, cy: 0.0, mpp: 1.0e9 },
            settings_dirty: false,
            view_ready: false,
            zoomed_for_ship: false,
            had_ship: false,
            zoom_pending: 0.0,
            offset: (0.0, 0.0),
            follow_selection: true,
            follow_paused: false,
            show_names: false,
            chat: ChatBox::default(),
            watch: None,
            last_target: Target::Free,
            last_target_pos: (0.0, 0.0),
            selected: None,
            heading: 0.0,
            thrust_pct: 100.0,
            mouse_aim: settings.mouse_aim,
            show_trails: false,
            trails_relative: true,
            show_prediction: true,
            show_shell_prediction: false,
            show_net,
            menu_open: false,
            ui_has_pointer: false,
            ui_has_keyboard: false,
            drag_from: None,
            drag_moved: 0.0,
            last_mouse: mouse_position(),
            frames: VecDeque::new(),
            last_frame_tick: 0,
            predictor: Predictor::new(),
            lookahead_ticks: (lookahead_seconds.clamp(2.0, 60.0) * 60.0) as u32,
            status: None,
            started: get_time(),
        })
    }

    fn say(&mut self, text: &str) {
        self.status = Some((text.to_string(), get_time() + 2.0));
    }

    pub fn frame(&mut self, settings: &mut Settings) -> Outcome {
        let dt = get_frame_time().min(0.1);
        // HUD panels scale with the window; things drawn in the world (ship, dots, labels) only mildly.
        let hud = settings.ui_factor();
        let ui = settings.marker_factor();
        let keys = !self.ui_has_keyboard && !self.menu_open && !self.chat.open;
        let mouse = mouse_position();

        // --- toggles -------------------------------------------------------------------------
        if is_key_pressed(KeyCode::Escape) {
            // Escape hides the suggestion list, then closes the chat line; otherwise the menu.
            if self.chat.open {
                self.chat.escape();
            } else {
                self.menu_open = !self.menu_open;
            }
        }
        if is_key_pressed(KeyCode::F3) {
            self.show_net = !self.show_net;
        }
        if is_key_pressed(KeyCode::F11) {
            settings.fullscreen = !settings.fullscreen;
            set_fullscreen(settings.fullscreen);
            settings.save();
        }
        if keys {
            if is_key_pressed(KeyCode::L) {
                self.show_trails = !self.show_trails;
                self.say(if self.show_trails { "Trails on" } else { "Trails off" });
            }
            if is_key_pressed(KeyCode::K) {
                self.trails_relative = !self.trails_relative;
                self.say(if self.trails_relative { "Trails relative to reference" } else { "Trails in world frame" });
            }
            if is_key_pressed(KeyCode::P) {
                self.show_prediction = !self.show_prediction;
            }
            if is_key_pressed(KeyCode::T) || is_key_pressed(KeyCode::Enter) {
                self.chat.open_with("");
            } else if is_key_pressed(KeyCode::Slash) {
                self.chat.open_with("/");
            }
            if is_key_pressed(KeyCode::G) {
                // Point at the spot under the cursor for everyone.
                let (x, y) = self.view.to_world(mouse.0, mouse.1);
                self.net.session.send_mark(x, y);
            }
            if is_key_pressed(KeyCode::N) {
                self.show_names = !self.show_names;
            }
            if is_key_pressed(KeyCode::O) {
                self.show_shell_prediction = !self.show_shell_prediction;
            }
            if is_key_pressed(KeyCode::F) {
                self.follow_selection = !self.follow_selection;
                self.watch = None;
                self.follow_paused = false;
                self.say(if self.follow_selection { "Following selection" } else { "Following own ship" });
            }
            if is_key_pressed(KeyCode::R) {
                self.offset = (0.0, 0.0);
            }
            if is_key_pressed(KeyCode::M) {
                self.mouse_aim = !self.mouse_aim;
                settings.mouse_aim = self.mouse_aim;
                settings.save();
                self.say(if self.mouse_aim { "Aim: mouse" } else { "Aim: A/D keys" });
            }
            if is_key_pressed(KeyCode::X) {
                self.thrust_pct = 0.0;
            }
            if is_key_pressed(KeyCode::Z) {
                self.thrust_pct = 100.0;
            }
            let shift = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
            let ctrl = is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl);
            if shift {
                self.thrust_pct = (self.thrust_pct + 80.0 * dt).min(100.0);
            } else if ctrl {
                self.thrust_pct = (self.thrust_pct - 80.0 * dt).max(0.0);
            }
        }

        // --- controls for this frame (from last frame's view of the world) ---------------------
        let mut controls = Controls::default();
        let now = self.net.now();
        let own = self
            .net
            .session
            .world
            .as_ref()
            .zip(self.net.session.present(now))
            .and_then(|(w, t)| w.ship_at(w.my_id, t.min(w.head as f64)));
        let (mwx, mwy) = self.view.to_world(mouse.0, mouse.1);
        if let Some(ship) = own {
            let bearing = (mwy - ship.y).atan2(mwx - ship.x);
            if self.mouse_aim {
                self.heading = bearing;
            } else if keys {
                if is_key_down(KeyCode::A) {
                    self.heading -= TURN_RATE * dt as f64;
                }
                if is_key_down(KeyCode::D) {
                    self.heading += TURN_RATE * dt as f64;
                }
            }
            controls.angle = math::dir_to_angle(self.heading.cos(), self.heading.sin());
            let burning = keys && (is_key_down(KeyCode::W) || is_key_down(KeyCode::Up));
            controls.thrust = if burning { self.thrust_pct.round() as u8 } else { 0 };
            if keys && is_key_down(KeyCode::Space) {
                let (angle, speed) = self.shell_aim(&ship, (mwx, mwy));
                controls.fire = Some((angle, speed as f32));
            }
        }

        self.net.update(controls);
        if let Some(reason) = self.net.session.rejected.clone() {
            return Outcome::ToMenu(format!("Join rejected: {reason}"));
        }
        if let Some(reason) = self.net.disconnect_reason() {
            return Outcome::ToMenu(format!("Disconnected: {reason}"));
        }
        clear_background(style::BACKGROUND);
        if self.net.session.world.is_none() {
            let waited = get_time() - self.started;
            let msg = if self.net.is_connected() { "Receiving world..." } else { "Connecting..." };
            style::text(msg, 40.0 * ui, 60.0 * ui, 30.0 * ui, WHITE);
            style::text("Esc to cancel", 40.0 * ui, 90.0 * ui, 20.0 * ui, GRAY);
            if is_key_pressed(KeyCode::Escape) || waited > 12.0 {
                return Outcome::ToMenu(if waited > 12.0 { "Connection timed out".into() } else { String::new() });
            }
            return Outcome::Continue;
        }

        let now = self.net.now();
        let present = self.net.session.present(now).unwrap_or(0.0);
        let fire_ready = self.net.session.fire_ready_tick();
        let stats = self.net.session.stats;
        let rtt = self.net.session.rtt();
        let lead = self.net.session.input_lead_ticks();
        let bytes = self.net.bytes_per_sec();
        let world = self.net.session.world.as_ref().unwrap();
        // Draw the newest instant for which every entity has a state on both sides.
        let tick_f = present.min(world.head as f64).max(0.0);
        let Some((row, tau)) = world.row_at(tick_f).or_else(|| world.row_at(world.head.saturating_sub(1) as f64)) else {
            style::text("Computing ephemeris...", 40.0 * ui, 60.0 * ui, 30.0 * ui, WHITE);
            return Outcome::Continue;
        };
        let body = |j: u32| World::body_at(&row, j as usize, tau);
        let alive = |j: u32| row.props.alive.get(j as usize).copied().unwrap_or(false);
        if let Some(gone) = self.selected.filter(|s| !alive(*s)) {
            // A merged body hands the selection to whatever absorbed it (looking back over the
            // last few seconds of merge events); anything else that vanished is deselected.
            let from = world.head.saturating_sub(4 * world.rules.tick_hz as Tick);
            self.selected = (from..world.head)
                .rev()
                .filter_map(|t| world.eph.get(t))
                .find_map(|r| r.merges.iter().find(|e| e.absorbed.contains(&gone)).map(|e| e.survivor))
                .flatten()
                .filter(|s| alive(*s));
        }
        let own = world.ship_at(world.my_id, tick_f);
        let me_ship = world.my_ship();

        // --- camera ----------------------------------------------------------------------------
        if self.had_ship && own.is_none() {
            // We just died: let go of whatever body the camera was following, so it stays at
            // the scene and then jumps to the new ship when it spawns.
            self.follow_paused = true;
        }
        self.had_ship = own.is_some();
        let watched = self.watch.and_then(|id| world.ship_at(id, tick_f).map(|p| (id, p)));
        if watched.is_none() {
            self.watch = None;
        }
        let target = match (self.follow_selection && !self.follow_paused, own, self.selected) {
            _ if watched.is_some() => Target::Player(watched.unwrap().0),
            (true, _, Some(s)) => Target::Body(s),
            (_, Some(_), _) => Target::Ship,
            _ => Target::Free,
        };
        let target_pos = match target {
            Target::Ship => own.map(|p| (p.x, p.y)).unwrap(),
            Target::Player(_) => watched.map(|(_, p)| (p.x, p.y)).unwrap(),
            Target::Body(s) => {
                let b = body(s);
                (b.x, b.y)
            }
            Target::Free => self.last_target_pos,
        };
        if !self.view_ready {
            // Start wide enough to see the neighbourhood.
            let extent = row.x.iter().zip(&row.y).fold(1.0e9f64, |m, (x, y)| m.max(x.abs()).max(y.abs()));
            self.view.mpp = (extent * 0.5 / screen_height().min(screen_width()) as f64).max(1.0);
            self.view_ready = true;
        } else if target != self.last_target {
            // Keep the picture still when the thing being followed changes, except when our
            // ship (re)appears: then the camera jumps to it.
            self.offset = if target == Target::Ship && self.last_target == Target::Free {
                (0.0, 0.0)
            } else {
                (self.view.cx - target_pos.0, self.view.cy - target_pos.1)
            };
        }
        if let (false, Some(ship)) = (self.zoomed_for_ship, own) {
            // First sight of our ship: frame the system out to the ship's own orbit.
            self.zoomed_for_ship = true;
            let (mut w, mut bx, mut by) = (0.0, 0.0, 0.0);
            for j in 0..row.x.len() {
                let m = row.props.mass[j].abs();
                w += m;
                bx += m * row.x[j];
                by += m * row.y[j];
            }
            if w > 0.0 {
                let d = ((ship.x - bx / w).powi(2) + (ship.y - by / w).powi(2)).sqrt();
                self.view.mpp = (d * 2.6 / screen_height().min(screen_width()) as f64).max(1.0);
            }
        }
        self.last_target = target;
        self.last_target_pos = target_pos;

        let pointer = !self.ui_has_pointer && !self.menu_open;
        let wheel = mouse_wheel().1;
        // While the chat is open the wheel belongs to it.
        if pointer && !self.chat.open && wheel != 0.0 {
            // Wheel units differ wildly between platforms (notches vs. pixel deltas), hence the
            // per-platform default speed and the cap on what one frame can contribute.
            self.zoom_pending += (wheel * settings.zoom_speed).clamp(-1.5, 1.5);
        }
        if self.zoom_pending.abs() > 1e-3 {
            let step = self.zoom_pending * (12.0 * dt).min(1.0);
            self.zoom_pending -= step;
            // Zoom about the cursor: the point under it stays put relative to what we follow.
            // Bring the view up to date first, or a frame of the target's motion leaks in.
            self.view.cx = target_pos.0 + self.offset.0;
            self.view.cy = target_pos.1 + self.offset.1;
            let before = self.view.to_world(mouse.0, mouse.1);
            self.view.mpp = (self.view.mpp * 1.1f64.powf(-step as f64)).clamp(0.05, 1.0e12);
            self.view.cx = target_pos.0 + self.offset.0;
            self.view.cy = target_pos.1 + self.offset.1;
            let after = self.view.to_world(mouse.0, mouse.1);
            self.offset.0 += before.0 - after.0;
            self.offset.1 += before.1 - after.1;
        }
        let dragging = is_mouse_button_down(MouseButton::Left) || is_mouse_button_down(MouseButton::Middle);
        if pointer && (is_mouse_button_pressed(MouseButton::Left) || is_mouse_button_pressed(MouseButton::Middle)) {
            self.drag_from = Some(mouse);
            self.drag_moved = 0.0;
        }
        if dragging && self.drag_from.is_some() {
            let (dx, dy) = (mouse.0 - self.last_mouse.0, mouse.1 - self.last_mouse.1);
            self.drag_moved += dx.abs() + dy.abs();
            if self.drag_moved > 10.0 * ui {
                self.offset.0 -= dx as f64 * self.view.mpp;
                self.offset.1 -= dy as f64 * self.view.mpp;
            }
        }
        self.last_mouse = mouse;
        self.view.cx = target_pos.0 + self.offset.0;
        self.view.cy = target_pos.1 + self.offset.1;
        let view = self.view;

        // A click (press and release without dragging) selects the body under the cursor.
        if is_mouse_button_released(MouseButton::Left) {
            if self.drag_from.take().is_some() && self.drag_moved <= 10.0 * ui && pointer {
                let mut best: Option<(u32, f32, f64)> = None;
                for j in 0..row.x.len() as u32 {
                    if !alive(j) {
                        continue;
                    }
                    let b = body(j);
                    let s = view.to_screen(b.x, b.y);
                    let r_px = (row.props.radius[j as usize] / view.mpp) as f32;
                    let d = ((s.0 - mouse.0).powi(2) + (s.1 - mouse.1).powi(2)).sqrt() - r_px;
                    let m = row.props.mass[j as usize].abs();
                    if d <= PICK_RADIUS_PX * ui && best.is_none_or(|(_, bd, bm)| d < bd - 2.0 || ((d - bd).abs() <= 2.0 && m > bm)) {
                        best = Some((j, d.max(0.0), m));
                    }
                }
                self.selected = best.map(|b| b.0);
                self.watch = None;
                self.follow_paused = false;
            }
        } else if !dragging {
            self.drag_from = None;
        }

        let mut labels = style::Labels::default();

        // --- trails ----------------------------------------------------------------------------
        let sample_tick = world.head / TRAIL_EVERY_TICKS * TRAIL_EVERY_TICKS;
        if sample_tick != self.last_frame_tick {
            self.last_frame_tick = sample_tick;
            if let Some(r) = world.eph.get(sample_tick) {
                let ships = world
                    .players
                    .iter()
                    .filter_map(|(id, p)| p.ship.as_ref()?.at(sample_tick).map(|s| (*id, s.p.x, s.p.y)))
                    .collect();
                self.frames.push_back(TrailFrame { row: r, ships });
                while self.frames.len() > TRAIL_FRAMES {
                    self.frames.pop_front();
                }
            }
        }
        // Reference frame for trails and predictions: the selected body, if any.
        let ref_slot = self.selected;
        let ref_now = ref_slot.map(|s| body(s));
        if self.show_trails {
            let reference = if self.trails_relative { ref_slot } else { None };
            draw_trails(&self.frames, &view, reference, ref_now, &row, world.my_id, ui);
        }

        // --- bodies ----------------------------------------------------------------------------
        let row_view = row.view();
        let frame = SystemFrame::of(&row_view);
        let escape_radius = world.rules.escape_radius;
        for j in 0..row.x.len() as u32 {
            if !alive(j) {
                continue;
            }
            let b = body(j);
            let s = view.to_screen(b.x, b.y);
            let r_px = (row.props.radius[j as usize] / view.mpp) as f32;
            if !view.on_screen(s, r_px.min(1.0e6) + 4.0) {
                continue;
            }
            let mass = row.props.mass[j as usize];
            // Bodies that are leaving the system for good fade out before the simulation drops them.
            let fade = if escape_radius > 0.0 {
                let (r, escaping) = frame.escape_state(&row_view, j as usize, world.rules.g);
                if escaping { (2.0 - 2.0 * r / escape_radius).clamp(0.12, 1.0) as f32 } else { 1.0 }
            } else {
                1.0
            };
            style::body(s, r_px, mass, j, fade, ui);
            if let Some(name) = world.names.get(&j).filter(|_| self.show_names && self.selected != Some(j)) {
                let size = LABEL * ui;
                let below = s.1 + r_px.max(1.1 * ui).min(4000.0) + size;
                labels.push(name.as_str(), s.0, below, size, style::alpha(style::DIM, fade), Rank::BodyName);
            }
        }

        // --- shells and effects ----------------------------------------------------------------
        let blast_px = (world.rules.shell_blast_radius / view.mpp) as f32;
        for (id, shell) in &world.shells {
            let Some(p) = world.shell_at(*id, tick_f) else { continue };
            let s = view.to_screen(p.x, p.y);
            if !view.on_screen(s, blast_px) {
                continue;
            }
            let armed = tick_f >= (shell.spawn_tick + world.rules.shell_arm_ticks as Tick) as f64;
            if blast_px > 2.0 {
                let a = if armed { 0.20 } else { 0.05 };
                style::disc(s.0, s.1, blast_px, Color::new(1.0, 0.47, 0.35, a));
                style::ring(s.0, s.1, blast_px, 1.0, Color::new(1.0, 0.47, 0.35, if armed { 0.8 } else { 0.25 }));
            }
            style::disc(s.0, s.1, 2.0 * ui, Color::from_rgba(255, 210, 160, 255));
        }
        for e in &world.effects {
            let age = (tick_f - e.tick as f64) / world.rules.tick_hz as f64;
            if !(0.0..1.5).contains(&age) {
                continue;
            }
            let sim_age = age * world.rules.time_scale();
            let s = view.to_screen(e.at.x + e.at.vx * sim_age, e.at.y + e.at.vy * sim_age);
            let fade = (1.0 - age / 1.5) as f32;
            match e.kind {
                EffectKind::ShellBlast => style::disc(s.0, s.1, blast_px.max(6.0), Color::new(1.0, 0.6, 0.3, 0.5 * fade)),
                EffectKind::Captured => {
                    // Two gold rings bursting outwards.
                    for delay in [0.0f32, 0.18] {
                        let t = (age as f32 - delay) / 1.2;
                        if (0.0..1.0).contains(&t) {
                            let r = (14.0 + 110.0 * t) * ui;
                            style::ring(s.0, s.1, r, (4.0 - 3.0 * t) * ui, style::alpha(style::GOLD, (1.0 - t) * (1.0 - t)));
                        }
                    }
                    style::disc(s.0, s.1, 26.0 * ui, style::alpha(style::GOLD, 0.35 * (1.0 - (age as f32 / 0.4).min(1.0))));
                }
                EffectKind::ShipDestroyed => {
                    let r = (10.0 + 60.0 * age as f32) * ui;
                    style::ring(s.0, s.1, r, 3.0 * ui, Color::new(1.0, 0.9, 0.5, fade));
                }
            }
        }

        // --- predictions -----------------------------------------------------------------------
        if self.predictor.poll() {
            let ship = me_ship.filter(|t| t.crashed.is_none());
            let wanted = self.show_prediction || self.show_shell_prediction;
            match (ship, world.eph.reader(), world.me()) {
                (Some(track), Some(reader), Some(me)) if wanted => {
                    let shell = own.filter(|_| self.show_shell_prediction).and_then(|ship| {
                        let (angle, speed) = self.shell_aim(&ship, (mwx, mwy));
                        world.shell_muzzle(angle, speed)
                    });
                    self.predictor.submit(Job {
                        reader,
                        rules: world.rules.clone(),
                        ship: *track.last(),
                        start: track.end(),
                        timeline: me.timeline.clone(),
                        ticks: self.lookahead_ticks,
                        show_ship: self.show_prediction,
                        held: controls.thrust > 0,
                        shell,
                        ref_slot,
                    });
                }
                _ => self.predictor.clear(),
            }
        }
        let paths = &self.predictor.latest;
        // A result computed for another reference frame would be drawn in the wrong place.
        let stale = paths.ref_slot != ref_slot;
        let anchor = ref_now.map_or((0.0, 0.0), |b| (b.x, b.y));
        if !stale {
            draw_path(&paths.held, anchor, &view, ui, Color::from_rgba(255, 220, 90, 150));
            draw_path(&paths.coast, anchor, &view, ui, Color::from_rgba(90, 255, 120, 170));
            draw_path(&paths.shell, anchor, &view, ui, Color::from_rgba(255, 130, 90, 170));
        }
        let shell_live = (!stale).then(|| paths.shell.get(world.rules.shell_arm_ticks as usize)).flatten();
        if let Some(q) = shell_live {
            // From here on the shell is live.
            let s = view.to_screen(anchor.0 + q.0, anchor.1 + q.1);
            style::ring(s.0, s.1, 4.0 * ui, 1.5 * ui, Color::from_rgba(255, 130, 90, 220));
        }
        if stale {
        } else if let (true, Some(q)) = (paths.coast_impact, paths.coast.last()) {
            let s = view.to_screen(anchor.0 + q.0, anchor.1 + q.1);
            draw_line(s.0 - 6.0 * ui, s.1 - 6.0 * ui, s.0 + 6.0 * ui, s.1 + 6.0 * ui, 2.0 * ui, RED);
            draw_line(s.0 - 6.0 * ui, s.1 + 6.0 * ui, s.0 + 6.0 * ui, s.1 - 6.0 * ui, 2.0 * ui, RED);
            labels.push("impact", s.0, s.1 + 20.0 * ui, LABEL * ui, RED, Rank::Approach);
        } else if let Some((i, d)) = paths.closest {
            if i > 0 && i + 1 < paths.coast.len() {
                let q = paths.coast[i];
                let s = view.to_screen(anchor.0 + q.0, anchor.1 + q.1);
                style::ring(s.0, s.1, 5.0 * ui, 1.5 * ui, Color::from_rgba(90, 255, 120, 255));
                let eta = i as f64 / world.rules.tick_hz as f64;
                let c = Color::from_rgba(150, 255, 170, 255);
                let lines = vec![format!("closest {}", fmt::distance(d)), format!("in {eta:.1} s")];
                labels.block(lines, s.0, s.1 + 18.0 * ui, LABEL * ui, c, Rank::Approach);
            }
        }

        // --- ship wakes ------------------------------------------------------------------------
        // A short fading tail behind every ship, in the same frame as the prediction line
        // (relative to the selected body, if any). The full trails replace it when switched on.
        let t0 = tick_f.floor() as Tick;
        if !self.show_trails {
            let span = (2.5 * world.rules.tick_hz as f64) as Tick;
            for (id, p) in &world.players {
                let Some(track) = p.ship.as_ref() else { continue };
                let color = if *id == world.my_id { style::OWN_SHIP } else { style::OTHER_SHIP };
                let newest = t0.min(track.end());
                let oldest = newest.saturating_sub(span).max(track.base);
                let mut prev: Option<(f32, f32)> = None;
                for t in (oldest..=newest).step_by(2) {
                    let Some(state) = track.at(t) else { continue };
                    let shift = match (ref_slot, ref_now) {
                        (Some(r), Some(now)) => world
                            .eph
                            .get(t)
                            .filter(|row| row.props.alive[r as usize])
                            .map(|row| (now.x - row.x[r as usize], now.y - row.y[r as usize])),
                        _ => Some((0.0, 0.0)),
                    };
                    let Some(shift) = shift else {
                        prev = None;
                        continue;
                    };
                    let s = view.to_screen(state.p.x + shift.0, state.p.y + shift.1);
                    if let Some(q) = prev {
                        let k = (t - oldest) as f32 / span.max(1) as f32;
                        if view.on_screen(q, 100.0) || view.on_screen(s, 100.0) {
                            draw_line(q.0, q.1, s.0, s.1, 1.5 * ui, style::alpha(color, 0.5 * k * k));
                        }
                    }
                    prev = Some(s);
                }
            }
        }

        // --- ships -----------------------------------------------------------------------------
        for (id, p) in &world.players {
            let Some(ship) = world.ship_at(*id, tick_f) else { continue };
            let s = view.to_screen(ship.x, ship.y);
            if !view.on_screen(s, 40.0 * ui) {
                continue;
            }
            let mine = *id == world.my_id;
            let input = p.timeline.at(t0);
            // Our own heading is shown instantly; the simulation follows a few ticks later.
            let facing = if mine { self.heading } else { math::angle_to_radians(input.angle) };
            let fuel = p.ship.as_ref().map_or(0, |t| t.last().fuel);
            let color = if mine { style::OWN_SHIP } else { style::OTHER_SHIP };
            draw_ship(s, facing as f32, color, input.thrust > 0 && fuel > 0, ui);
            if !mine {
                labels.push(p.name.as_str(), s.0, s.1 + 20.0 * ui, LABEL * ui, color, Rank::Pilot);
            }
        }

        // --- objective -------------------------------------------------------------------------
        let gold = style::GOLD;
        let target = world.target.filter(|t| alive(*t) && world.next_round_tick.is_none());
        let mut orbit = None;
        if let Some(slot) = target {
            let b = body(slot);
            let radius = row.props.radius[slot as usize];
            let s = view.to_screen(b.x, b.y);
            let r_px = (radius / view.mpp) as f32;
            if view.on_screen(s, 0.0) {
                // The band a qualifying orbit must stay inside.
                let outer = (world.rules.orbit_max_apo_radii * radius / view.mpp) as f32;
                let inner = (world.rules.orbit_min_peri_radii * radius / view.mpp) as f32;
                if outer > 14.0 * ui {
                    style::ring(s.0, s.1, outer.min(1.0e5), ui, Color::new(1.0, 0.78, 0.24, 0.35));
                    style::ring(s.0, s.1, inner.min(1.0e5), ui, Color::new(1.0, 0.78, 0.24, 0.35));
                }
                let ring = r_px.min(4000.0) + 9.0 * ui;
                // Soft halo that breathes slowly.
                let pulse = 0.75 + 0.25 * (get_time() as f32 * 2.2).sin();
                for k in 1..=4 {
                    let a = 0.16 * pulse / k as f32;
                    style::ring(s.0, s.1, ring + 2.0 * k as f32 * ui, 2.5 * ui, style::alpha(gold, a));
                }
                style::ring(s.0, s.1, ring, 2.0 * ui, gold);
                labels.push("TARGET", s.0, s.1 - ring - 5.0 * ui, LABEL * ui, gold, Rank::Target);
            } else {
                // Off screen: an arrow on the edge pointing at it.
                let (cx, cy) = (screen_width() * 0.5, screen_height() * 0.5);
                let (dx, dy) = (s.0 - cx, s.1 - cy);
                let k = ((cx - 24.0 * ui) / dx.abs().max(1e-3)).min((cy - 24.0 * ui) / dy.abs().max(1e-3));
                let (ex, ey) = (cx + dx * k, cy + dy * k);
                let a = dy.atan2(dx);
                let tip = |fwd: f32, side: f32| vec2(ex + a.cos() * fwd - a.sin() * side, ey + a.sin() * fwd + a.cos() * side);
                draw_triangle(tip(10.0 * ui, 0.0), tip(-6.0 * ui, 7.0 * ui), tip(-6.0 * ui, -7.0 * ui), gold);
                if let Some(ship) = own {
                    let d = ((b.x - ship.x).powi(2) + (b.y - ship.y).powi(2)).sqrt();
                    labels.push(fmt::distance(d), ex - a.cos() * 34.0 * ui, ey - a.sin() * 34.0 * ui + 4.0 * ui, LABEL * ui, gold, Rank::Target);
                }
            }
            if let Some(ship) = own {
                orbit = Some((orbit_status(&ship, &b, row.props.mass[slot as usize], radius, &world.rules), radius));
            }
        }

        // --- selection info --------------------------------------------------------------------
        if let (Some(slot), Some(b)) = (self.selected, ref_now) {
            let s = view.to_screen(b.x, b.y);
            let r_px = ((row.props.radius[slot as usize] / view.mpp) as f32).min(4000.0) + 6.0 * ui;
            draw_rectangle_lines(s.0 - r_px, s.1 - r_px, 2.0 * r_px, 2.0 * r_px, 1.5 * ui, WHITE);
            // The name line on top of the label is optional (N).
            let mut lines = Vec::new();
            if self.show_names {
                lines.push(world.body_name(slot));
            }
            lines.push(format!("m = {}", fmt::mass(row.props.mass[slot as usize])));
            lines.push(format!("r = {}", fmt::distance(row.props.radius[slot as usize])));
            if let Some(ship) = own {
                let d = ((b.x - ship.x).powi(2) + (b.y - ship.y).powi(2)).sqrt();
                let v = ((b.vx - ship.vx).powi(2) + (b.vy - ship.vy).powi(2)).sqrt();
                lines.push(format!("d = {}", fmt::distance(d)));
                lines.push(format!("rel v = {}", fmt::speed(v)));
            }
            labels.block(lines, s.0, s.1 + r_px + 12.0 * ui, LABEL * ui, WHITE, Rank::Selection);
        }

        // --- pings -----------------------------------------------------------------------------
        for m in &self.net.session.marks {
            let age = (now - m.at) as f32;
            if !(0.0..6.0).contains(&age) {
                continue;
            }
            let s = view.to_screen(m.x, m.y);
            let fade = (1.0 - age / 6.0).min(1.0);
            // A ring that keeps rippling outwards while the marker lasts.
            let ripple = (age * 1.2).fract();
            style::ring(s.0, s.1, (6.0 + 22.0 * ripple) * ui, 1.5 * ui, style::alpha(style::ACCENT, fade * (1.0 - ripple)));
            style::disc(s.0, s.1, 2.5 * ui, style::alpha(style::ACCENT, fade));
            labels.push(m.name.as_str(), s.0, s.1 - 10.0 * ui, LABEL * ui, style::alpha(style::ACCENT, fade), Rank::Pilot);
        }

        labels.draw();

        // --- HUD -------------------------------------------------------------------------------
        let me = world.me();
        let fuel = me.and_then(|m| m.ship.as_ref()).map(|t| t.last().fuel);
        let respawn_in = me.and_then(|m| m.respawn_tick).map(|t| (t as f64 - tick_f).max(0.0) / world.rules.tick_hz as f64);
        let reload = ((fire_ready as f64 - tick_f) / world.rules.tick_hz as f64).max(0.0);
        let mut scores: Vec<(String, u32, u32, u32, u32, bool)> = world
            .players
            .iter()
            .map(|(id, p)| (p.name.clone(), p.score(&world.rules), p.kills, p.deaths, p.captures, *id == world.my_id))
            .collect();
        scores.sort_by(|a, b| b.1.cmp(&a.1).then(a.3.cmp(&b.3)));
        let secs = |t: Tick| (t as f64 - tick_f).max(0.0) / world.rules.tick_hz as f64;
        let round_left = world.round_end_tick.map(secs);
        let intermission = world.next_round_tick.map(secs);
        let hold_goal = world.rules.hold_ticks as f32;
        let my_hold = world.holds.get(&world.my_id).copied().unwrap_or(0) as f32 / hold_goal;
        let rivals: Vec<(String, f32)> = world
            .holds
            .iter()
            .filter(|(id, _)| **id != world.my_id)
            .map(|(id, h)| (world.player_name(*id).to_string(), *h as f32 / hold_goal))
            .collect();
        let target_name = target.map(|t| world.body_name(t));
        let limits = (world.rules.orbit_max_ecc, world.rules.orbit_min_peri_radii, world.rules.orbit_max_apo_radii);
        let capture_points = world.rules.capture_points;
        // What the chat box needs: names to complete and to highlight.
        let player_names: Vec<String> = world.players.values().map(|p| p.name.clone()).collect();
        let mut mentions: Vec<(String, Mention)> = world.players.iter().map(|(id, p)| (p.name.clone(), Mention::Player(*id))).collect();
        mentions.extend(world.names.iter().map(|(slot, name)| (name.clone(), Mention::Body(*slot))));
        // Unnamed bodies are worth mentioning when they are the target or selected.
        for slot in [target, self.selected].into_iter().flatten() {
            if !world.names.contains_key(&slot) {
                mentions.push((world.body_name(slot), Mention::Body(slot)));
            }
        }
        let body_names: Vec<String> = mentions.iter().filter(|m| matches!(m.1, Mention::Body(_))).map(|m| m.0.clone()).collect();
        let presets: Vec<&str> = PRESETS.iter().map(|p| p.0).collect();
        let complete_ctx = Context { players: &player_names, bodies: &body_names, presets: &presets, op: self.net.session.op };
        let my_id = world.my_id;
        let alive_bodies = row.props.alive.iter().filter(|a| **a).count();
        let clock = |s: f64| format!("{}:{:02}", s as u32 / 60, s as u32 % 60);
        let title = format!("{}   ·   ROUND {}", world.preset.to_uppercase(), world.round);
        let horizon = (paths.coast.len().max(1) - 1) as f64 / world.rules.tick_hz as f64;
        let predict_ms = paths.compute_ms;
        let net_lines: Vec<String> = vec![
            format!("{} bodies   time x{:.0}   {} fps", alive_bodies, world.rules.time_scale(), get_fps()),
            format!("tick {:.1}  head {}  ephemeris +{} ticks", tick_f, world.head, world.eph.end_tick().unwrap_or(0).saturating_sub(world.head)),
            format!("rtt {:.0} ms   input lead {} ticks ({:.0} ms)", rtt.unwrap_or(0.0) * 1e3, lead, lead as f64 * 1e3 / world.rules.tick_hz as f64),
            format!("world hash: {} checks, {} mismatches, {} resyncs", stats.hash_checks, stats.hash_mismatches, stats.resyncs),
            format!("ship checks: {} ok, {} corrected", stats.ship_checks - stats.ship_corrections, stats.ship_corrections),
            format!("my inputs: {} sent, {} re-timed by server", stats.cmds_sent, stats.retimed_cmds),
            format!("remote inputs in my past: {}  (ship replays {})", stats.late_remote_inputs, stats.rollbacks),
            format!("net {:.1} kB/s up, {:.1} kB/s down", bytes.0 / 1e3, bytes.1 / 1e3),
            format!("prediction {horizon:.1} s ahead, computed in {predict_ms:.0} ms"),
        ];
        let status = self.status.as_ref().filter(|s| s.1 > get_time()).map(|s| s.0.clone());
        let fuel_max = world.rules.fuel_max_mmps;
        let cooldown = world.rules.shell_cooldown_ticks as f64 / world.rules.tick_hz as f64;
        let thrust_pct = self.thrust_pct;
        let mouse_aim = self.mouse_aim;
        // The ruler replaces a "metres per pixel" readout.
        style::scale_bar(screen_width() * 0.5, screen_height() - 18.0 * hud, view.mpp, fmt::distance_round, hud);

        let mut outcome = Outcome::Continue;
        let (mut trails, mut trails_rel, mut pred, mut shell_pred, mut follow_sel, mut names, mut net_dbg, mut menu) = (
            self.show_trails,
            self.trails_relative,
            self.show_prediction,
            self.show_shell_prediction,
            self.follow_selection,
            self.show_names,
            self.show_net,
            self.menu_open,
        );
        let (mut has_ptr, mut has_kb) = (false, false);
        let mut chat_out = chat::Outcome::default();
        let (mut ui_scale, mut zoom_speed) = (settings.ui_scale, settings.zoom_speed);
        egui_macroquad::ui(|ctx| {
            use egui::{Align2, Area, Id, RichText};
            ctx.set_zoom_factor(hud);
            let (gold, dim, good, bad) = (style::c32(style::GOLD), style::c32(style::DIM), style::c32(style::GOOD), style::c32(style::EMBER));

            // Top centre: where we are and how long is left.
            Area::new(Id::new("round")).anchor(Align2::CENTER_TOP, [0.0, 10.0]).show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(&title).small().color(dim).extra_letter_spacing(2.0));
                    match (intermission, round_left) {
                        (Some(left), _) => {
                            let verdict = match scores.first() {
                                Some(best) if best.1 > 0 => format!("{} wins with {}", best.0, best.1),
                                _ => "Nobody scored".to_string(),
                            };
                            ui.label(RichText::new(verdict).heading().color(gold));
                            ui.label(RichText::new(format!("new world in {left:.0} s")).color(dim));
                        }
                        (None, Some(left)) => {
                            let colour = if left < 30.0 { bad } else { style::c32(style::TEXT) };
                            ui.label(RichText::new(clock(left)).heading().color(colour));
                        }
                        _ => {}
                    }
                    // The objective, compact: where to orbit, which conditions hold, progress.
                    if let Some(name) = &target_name {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            // Centre the row by hand: egui lays horizontal rows out from the left.
                            ui.add_space((ui.available_width() - 250.0).max(0.0) * 0.5);
                            style::caption(ui, "ORBIT");
                            ui.label(RichText::new(format!("{name}  +{capture_points}")).color(gold).strong());
                            let mark = |ui: &mut egui::Ui, what: &str, ok: Option<bool>| {
                                let colour = match ok {
                                    Some(true) => good,
                                    Some(false) => bad,
                                    None => dim,
                                };
                                ui.label(RichText::new(what).small().color(colour).extra_letter_spacing(1.0));
                            };
                            let state = orbit.filter(|(o, _)| o.bound);
                            mark(ui, "ECC", state.map(|(o, _)| o.ecc <= limits.0));
                            mark(ui, "LOW", state.map(|(o, r)| o.peri / r >= limits.1));
                            mark(ui, "HIGH", state.map(|(o, r)| o.apo / r <= limits.2));
                        });
                        ui.allocate_ui(egui::vec2(220.0, 0.0), |ui| {
                            style::gauge(ui, "HOLD", &format!("{:.0} %", my_hold.min(1.0) * 100.0), my_hold, style::GOLD);
                            for (rival, frac) in &rivals {
                                style::gauge(ui, &rival.to_uppercase(), &format!("{:.0} %", frac.min(1.0) * 100.0), *frac, style::OTHER_SHIP);
                            }
                        });
                    }
                    if let Some(s) = &status {
                        ui.label(RichText::new(s).color(gold));
                    }
                });
            });

            // Top left: diagnostics, only on request.
            if net_dbg {
                Area::new(Id::new("net")).anchor(Align2::LEFT_TOP, [10.0, 10.0]).show(ctx, |ui| {
                    style::panel().show(ui, |ui| {
                        for l in &net_lines {
                            ui.label(RichText::new(l).monospace().color(dim));
                        }
                    });
                });
            }

            // Top right: standings and what just happened.
            Area::new(Id::new("score")).anchor(Align2::RIGHT_TOP, [-10.0, 10.0]).show(ctx, |ui| {
                style::panel().show(ui, |ui| {
                    egui::Grid::new("scores").num_columns(5).spacing([14.0, 3.0]).show(ui, |ui| {
                        for head in ["PILOT", "PTS", "ORB", "K", "D"] {
                            style::caption(ui, head);
                        }
                        ui.end_row();
                        for (name, score, k, d, caps, mine) in &scores {
                            let colour = if *mine { style::c32(style::OWN_SHIP) } else { style::c32(style::TEXT) };
                            ui.label(RichText::new(name).color(colour));
                            ui.label(RichText::new(score.to_string()).color(colour).strong());
                            for n in [caps, k, d] {
                                ui.label(RichText::new(n.to_string()).color(dim));
                            }
                            ui.end_row();
                        }
                    });
                });
            });

            // Bottom right: the ship.
            Area::new(Id::new("ship")).anchor(Align2::RIGHT_BOTTOM, [-10.0, -10.0]).show(ctx, |ui| {
                style::panel().show(ui, |ui| {
                    ui.set_width(220.0);
                    match fuel {
                        Some(f) => {
                            let frac = f as f32 / fuel_max as f32;
                            let fuel_colour = if frac < 0.2 { style::EMBER } else { style::ACCENT };
                            style::gauge(ui, "DELTA-V", &fmt::speed(f as f64 / 1000.0), frac, fuel_colour);
                            style::gauge(ui, "THROTTLE", &format!("{thrust_pct:.0} %"), thrust_pct / 100.0, style::GOOD);
                            let loaded = (1.0 - reload / cooldown.max(1e-6)).clamp(0.0, 1.0) as f32;
                            let shell = if reload > 0.0 { format!("{reload:.1} s") } else { "ready".to_string() };
                            style::gauge(ui, "SHELL", &shell, loaded, if reload > 0.0 { style::DIM } else { style::EMBER });
                            style::caption(ui, if mouse_aim { "AIM  MOUSE  (M)" } else { "AIM  A / D  (M)" });
                        }
                        None => {
                            style::caption(ui, "SHIP LOST");
                            ui.label(RichText::new(match respawn_in {
                                Some(s) => format!("respawn in {s:.1} s"),
                                None => "waiting for a ship".into(),
                            })
                            .heading()
                            .color(bad));
                        }
                    }
                });
            });

            chat_out = self.chat.show(ctx, &self.net.session.chat, now, &complete_ctx, &mentions);
            if menu {
                egui::Window::new("MENU").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
                    ctx,
                    |ui| {
                        ui.set_width(330.0);
                        style::section(ui, "VIEW");
                        style::toggle(ui, &mut pred, "Ship trajectory", "P");
                        style::toggle(ui, &mut shell_pred, "Shell trajectory preview", "O");
                        style::toggle(ui, &mut trails, "Trails", "L");
                        style::toggle(ui, &mut trails_rel, "Trails relative to selection", "K");
                        style::toggle(ui, &mut follow_sel, "Camera follows selection", "F");
                        style::toggle(ui, &mut net_dbg, "Network details", "F3");
                        style::toggle(ui, &mut names, "Body names", "N");
                        style::section(ui, "INTERFACE");
                        ui.add(egui::Slider::new(&mut ui_scale, 0.6..=2.5).text("size"));
                        ui.add(egui::Slider::new(&mut zoom_speed, 0.002..=3.0).logarithmic(true).text("zoom speed"));
                        style::section(ui, "CONTROLS");
                        style::key_row(ui, "Thrust", "W / UP");
                        style::key_row(ui, "Throttle, cut, full", "SHIFT / CTRL, X, Z");
                        style::key_row(ui, "Fire towards cursor", "SPACE");
                        style::key_row(ui, "Aim with mouse or A / D", "M");
                        style::key_row(ui, "Zoom, pan, select", "WHEEL, DRAG, CLICK");
                        style::key_row(ui, "Chat, command", "T, /");
                        style::key_row(ui, "Point at the map", "G");
                        style::key_row(ui, "Recentre, fullscreen", "R, F11");
                        style::section(ui, "MASS");
                        style::mass_legend(ui);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Resume").clicked() {
                                menu = false;
                            }
                            if ui.button("Leave game").clicked() {
                                outcome = Outcome::ToMenu(String::new());
                            }
                            if ui.button("Quit").clicked() {
                                outcome = Outcome::Quit;
                            }
                        });
                    },
                );
            }
            has_ptr = ctx.wants_pointer_input() || ctx.is_pointer_over_area();
            has_kb = ctx.wants_keyboard_input();
        });
        egui_macroquad::draw();
        if let Some(text) = chat_out.send {
            self.net.session.send_chat(&text);
        }
        match chat_out.clicked {
            Some(Mention::Body(slot)) => {
                self.selected = Some(slot);
                self.follow_paused = false;
                self.watch = None;
            }
            Some(Mention::Player(id)) => self.watch = Some(id).filter(|id| *id != my_id),
            None => {}
        }
        if ui_scale != settings.ui_scale || zoom_speed != settings.zoom_speed {
            settings.ui_scale = ui_scale;
            settings.zoom_speed = zoom_speed;
            self.settings_dirty = true;
        } else if self.settings_dirty && !is_mouse_button_down(MouseButton::Left) {
            self.settings_dirty = false;
            settings.save();
        }
        self.show_trails = trails;
        self.trails_relative = trails_rel;
        self.show_prediction = pred;
        self.show_shell_prediction = shell_pred;
        self.follow_selection = follow_sel;
        self.show_names = names;
        self.show_net = net_dbg;
        self.menu_open = menu;
        self.ui_has_pointer = has_ptr;
        self.ui_has_keyboard = has_kb;
        if !matches!(outcome, Outcome::Continue) {
            self.net.disconnect();
        }
        outcome
    }

    /// Shell heading and muzzle speed for the current cursor: the further the cursor, the faster.
    fn shell_aim(&self, ship: &Particle, mouse_world: (f64, f64)) -> (u16, f64) {
        let (dx, dy) = (mouse_world.0 - ship.x, mouse_world.1 - ship.y);
        let dist = (dx * dx + dy * dy).sqrt();
        let scale = self.net.session.world.as_ref().map_or(1.0, |w| w.rules.time_scale());
        // Cursor distance reads as "metres per real second".
        (math::dir_to_angle(dx, dy), dist / scale)
    }
}


fn draw_path(points: &[(f64, f64)], anchor: (f64, f64), view: &View, ui: f32, color: Color) {
    let mut prev: Option<(f32, f32)> = None;
    for q in points {
        let s = view.to_screen(anchor.0 + q.0, anchor.1 + q.1);
        if let Some(p) = prev {
            if (view.on_screen(p, 200.0) || view.on_screen(s, 200.0)) && s.0.is_finite() && s.1.is_finite() {
                draw_line(p.0, p.1, s.0, s.1, 1.5 * ui, color);
            }
        }
        prev = Some(s);
    }
}

fn draw_trails(
    frames: &VecDeque<TrailFrame>,
    view: &View,
    reference: Option<u32>,
    ref_now: Option<Particle>,
    now_row: &EphRow,
    my_id: PlayerId,
    ui: f32,
) {
    if frames.len() < 2 {
        return;
    }
    // Each historic point is drawn where it was relative to the reference at that time.
    let shift = |f: &TrailFrame| match (reference, ref_now) {
        (Some(r), Some(now)) if f.row.props.alive[r as usize] => (now.x - f.row.x[r as usize], now.y - f.row.y[r as usize]),
        _ => (0.0, 0.0),
    };
    let shifts: Vec<(f64, f64)> = frames.iter().map(shift).collect();
    let body_color = Color::from_rgba(130, 160, 255, 70);
    for j in 0..now_row.x.len() {
        if !now_row.props.alive[j] {
            continue;
        }
        // Skip bodies that are nowhere near the screen right now.
        let newest = frames.back().unwrap();
        let sh = shifts[frames.len() - 1];
        if !view.on_screen(view.to_screen(newest.row.x[j] + sh.0, newest.row.y[j] + sh.1), 300.0) {
            continue;
        }
        let mut prev: Option<(f32, f32)> = None;
        for (f, sh) in frames.iter().zip(&shifts) {
            if !f.row.props.alive[j] {
                prev = None;
                continue;
            }
            let s = view.to_screen(f.row.x[j] + sh.0, f.row.y[j] + sh.1);
            if let Some(p) = prev {
                draw_line(p.0, p.1, s.0, s.1, ui, body_color);
            }
            prev = Some(s);
        }
    }
    let mut ids: Vec<PlayerId> = frames.iter().flat_map(|f| f.ships.iter().map(|s| s.0)).collect();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        let color = if id == my_id { Color::from_rgba(255, 225, 150, 130) } else { Color::from_rgba(130, 200, 255, 130) };
        let mut prev: Option<(f32, f32)> = None;
        for (f, sh) in frames.iter().zip(&shifts) {
            let Some(p) = f.ships.iter().find(|s| s.0 == id) else {
                prev = None;
                continue;
            };
            let s = view.to_screen(p.1 + sh.0, p.2 + sh.1);
            if let Some(q) = prev {
                // A respawn teleports the ship: do not join the two lives with a line.
                if (q.0 - s.0).abs() + (q.1 - s.1).abs() < 400.0 {
                    draw_line(q.0, q.1, s.0, s.1, 1.5 * ui, color);
                }
            }
            prev = Some(s);
        }
    }
}

/// A small dart with swept wings, a canopy and an engine plume. `s` is the screen position,
/// `facing` the heading in radians (screen space, y down).
fn draw_ship(s: (f32, f32), facing: f32, color: Color, burning: bool, ui: f32) {
    let size = 6.0 * ui;
    let (c, n) = (facing.cos(), facing.sin());
    // Ship-local coordinates: x forward, y to the side, in units of `size`.
    let at = |fwd: f32, side: f32| vec2(s.0 + (c * fwd - n * side) * size, s.1 + (n * fwd + c * side) * size);
    let shade = |k: f32, a: f32| Color::new(color.r * k, color.g * k, color.b * k, a);

    if burning {
        // Flickering plume: a wide soft flame with a hot core.
        let t = get_time() as f32;
        let flick = 0.75 + 0.25 * (t * 47.0).sin() * (t * 31.0).cos();
        let len = 2.6 * flick;
        draw_triangle(at(-0.75, 0.34), at(-0.75, -0.34), at(-0.9 - len, 0.0), Color::new(1.0, 0.45, 0.15, 0.85));
        draw_triangle(at(-0.75, 0.18), at(-0.75, -0.18), at(-0.9 - 0.55 * len, 0.0), Color::new(1.0, 0.92, 0.6, 0.95));
    }
    // Wings, swept back, darker than the hull.
    for side in [1.0f32, -1.0] {
        draw_triangle(at(0.45, 0.22 * side), at(-1.25, 1.2 * side), at(-0.8, 0.22 * side), shade(0.62, 1.0));
        // Wingtip accent.
        draw_triangle(at(-0.95, 1.02 * side), at(-1.25, 1.2 * side), at(-1.45, 1.12 * side), shade(1.0, 1.0));
    }
    // Hull: a long nose tapering from the engine block.
    draw_triangle(at(2.1, 0.0), at(-0.9, 0.5), at(-0.9, -0.5), color);
    // Spine highlight and canopy.
    draw_triangle(at(2.1, 0.0), at(-0.9, 0.0), at(-0.9, -0.5), shade(0.8, 1.0));
    draw_triangle(at(1.2, 0.0), at(0.2, 0.2), at(0.2, -0.2), Color::new(0.55, 0.85, 1.0, 1.0));
    // Engine nozzle.
    draw_triangle(at(-0.9, 0.3), at(-0.9, -0.3), at(-0.55, 0.0), Color::new(0.1, 0.12, 0.16, 1.0));
}


impl Game {
    /// Tell the server we are going, so our name and ship are released at once.
    pub fn leave(&mut self) {
        self.net.disconnect();
    }
}
