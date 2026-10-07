//! Large-scale single player: one ship in a world of hundreds of thousands of bodies run by
//! `gsim-swarm`. No server, no rounds, no score; time can be slowed, stopped and sped up.

use crate::chat::{self, ChatBox, Mention};
use crate::density::{Big, ColorMode, Density, Flash};
use crate::fmt;
use crate::game::{draw_path, frame_both, draw_ship, Outcome, View, LABEL, PICK_RADIUS_PX, TURN_RATE};
use crate::predictor::{Job, Predictor};
use crate::settings::Settings;
use crate::style::{self, Rank};
use egui_macroquad::egui;
use gsim_client_core::complete::Context;
use gsim_client_core::eph::Eph;
use gsim_client_core::ChatEntry;
use gsim_core::{math, GameRules, InputTimeline, Particle, ShipInput, ShipState};
use gsim_proto::command;
use gsim_proto::ChatKind;
use gsim_swarm::runner::{Command, Heavy, LocalWorld, Runner, Stats};
use gsim_swarm::scenario::Setup;
use gsim_swarm::sim::Event;
use gsim_swarm::{Merge, Sim};
use macroquad::prelude::*;
use rayon::prelude::*;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Ship,
    Body(u32),
    Free,
}

/// The ship's surroundings being simulated ahead for the prediction line.
struct Ahead {
    eph: Eph,
    world: Arc<LocalWorld>,
    upto: u64,
    /// The pace it was computed for: a different step length makes it worthless.
    pace: f64,
}

/// What the simulation thread last published, copied out under its lock.
struct Seen {
    clock: gsim_swarm::runner::Clock,
    ship: Option<ShipState>,
    ship_before: Option<ShipState>,
    ship_tick: u64,
    respawn_tick: Option<u64>,
    tick: u64,
    god: bool,
    pace: f64,
    ship_dt: f64,
    time: f64,
    theta: f32,
    stats: Stats,
    heaviest: Vec<Heavy>,
    local: Option<Arc<LocalWorld>>,
    watch: Vec<u32>,
}

/// A selected body as drawn this frame.
#[derive(Clone, Copy)]
struct Picked {
    at: Particle,
    mass: f64,
    radius: f64,
}

/// Time buttons: simulated seconds per real second.
const PACES: [(f64, &str); 7] = [(0.0, "||"), (3600.0, "1h"), (21_600.0, "6h"), (86_400.0, "1d"), (172_800.0, "2d"), (345_600.0, "4d"), (691_200.0, "8d")];
/// Holding a speed key doubles or halves the pace this many times a second.
const SPEED_KEY_RATE: f64 = 1.5;
const FLASH_TICKS: f64 = 36.0;
const WAKE_POINTS: usize = 80;

pub struct Sandbox {
    runner: Runner,
    rules: GameRules,
    scenario: String,
    player: String,
    view: View,
    view_ready: bool,
    zoomed_for_ship: bool,
    had_ship: bool,
    zoom_pending: f32,
    offset: (f64, f64),
    last_target: Target,
    last_target_pos: (f64, f64),
    selected: Option<u32>,
    /// Tick at which the simulation was last told what to watch.
    watch_told: u64,
    heading: f64,
    thrust_pct: f32,
    menu_open: bool,
    ui_has_pointer: bool,
    ui_has_keyboard: bool,
    settings_dirty: bool,
    drag_from: Option<(f32, f32)>,
    drag_moved: f32,
    last_mouse: (f32, f32),
    chat: ChatBox,
    log: VecDeque<ChatEntry>,
    density: Density,
    meter: fmt::Meter,
    flashes: Vec<(Merge, u64)>,
    /// Where ships were lost: position, velocity of the wreck, tick.
    wrecks: Vec<(f64, f64, f64, f64, u64)>,
    predictor: Predictor,
    ahead: Option<Ahead>,
    next_ahead: Option<Ahead>,
    lookahead_ticks: u32,
    /// Recent ship positions relative to the selected body (or absolute), newest last.
    wake: VecDeque<(f64, f64)>,
    wake_tick: u64,
    wake_ref: Option<u32>,
    last_ship: Option<Particle>,
    /// Simulated seconds per real second last asked for (kept while paused).
    last_pace: f64,
    pace_changed: Instant,
    status: Option<(String, f64)>,
    started: Instant,
}

impl Sandbox {
    pub fn start(setup: Setup, seed: u64, settings: &Settings, lookahead_seconds: f32) -> Self {
        let scenario = setup.name.clone();
        let sim = Sim::new(setup, seed);
        let rules = sim.rules.clone();
        let pace = rules.time_scale();
        let log = VecDeque::new();
        let mut density = Density::new();
        density.set_gpu(settings.gpu);
        Self {
            runner: Runner::start(sim, crate::density::simulation_threads(settings.gpu)),
            rules,
            scenario,
            player: settings.name.trim().to_string(),
            view: View { cx: 0.0, cy: 0.0, mpp: 1.0e9 },
            view_ready: false,
            zoomed_for_ship: false,
            had_ship: false,
            zoom_pending: 0.0,
            offset: (0.0, 0.0),
            last_target: Target::Free,
            last_target_pos: (0.0, 0.0),
            selected: None,
            watch_told: 0,
            heading: 0.0,
            thrust_pct: 100.0,
            menu_open: false,
            ui_has_pointer: false,
            ui_has_keyboard: false,
            settings_dirty: false,
            drag_from: None,
            drag_moved: 0.0,
            last_mouse: mouse_position(),
            chat: ChatBox::default(),
            log,
            density,
            meter: fmt::Meter::default(),
            flashes: Vec::new(),
            wrecks: Vec::new(),
            predictor: Predictor::new(),
            ahead: None,
            next_ahead: None,
            lookahead_ticks: (lookahead_seconds.clamp(2.0, 60.0) * 60.0) as u32,
            wake: VecDeque::new(),
            wake_tick: 0,
            wake_ref: None,
            last_ship: None,
            last_pace: pace,
            pace_changed: Instant::now(),
            status: None,
            started: Instant::now(),
        }
    }

    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn say(&mut self, text: &str) {
        self.status = Some((text.to_string(), get_time() + 2.0));
    }

    fn print(&mut self, kind: ChatKind, text: impl Into<String>) {
        self.log.push_back(ChatEntry { at: self.now(), kind, from: None, text: text.into() });
        while self.log.len() > 200 {
            self.log.pop_front();
        }
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

    fn select(&mut self, id: Option<u32>, tick: u64) {
        self.selected = id;
        self.watch_told = tick;
        self.runner.send(Command::Watch(id.into_iter().collect()));
    }

    /// Set the pace in simulated seconds per real second (0 pauses). There are no limits:
    /// a huge value takes huge, crude steps, and that is the player's call.
    fn set_pace(&mut self, pace: f64) {
        if pace > 0.0 && pace != self.last_pace {
            self.last_pace = pace;
            self.pace_changed = Instant::now();
        }
        self.runner.send(Command::Pace(pace));
    }

    /// The rules as they are at the current pace (the step length follows it).
    fn rules_now(&self) -> GameRules {
        GameRules::new(self.last_pace, self.rules.tick_hz)
    }

    /// Run a chat line here: there is no server to send it to.
    fn command(&mut self, line: &str, selected: Option<Picked>) {
        let Some(rest) = line.strip_prefix('/') else {
            let from = Some(self.player.clone());
            self.log.push_back(ChatEntry { at: self.now(), kind: ChatKind::Say, from, text: line.to_string() });
            return;
        };
        let words = command::split(rest);
        let Some(name) = words.first() else { return };
        let Some(spec) = command::find(name).filter(|c| c.available(true)) else {
            return self.print(ChatKind::Error, format!("Unknown command: /{name}"));
        };
        // Naming yourself is allowed wherever a player may be named.
        let me = |w: &String| w == "@s" || w == "@a" || w == "@r" || w.eq_ignore_ascii_case(&self.player);
        let args: Vec<&String> = words[1..].iter().skip_while(|w| me(w)).collect();
        // An empty error stands for "show the usage line", as on a server.
        let result: Result<String, String> = match spec.name {
            "help" => {
                for c in command::COMMANDS.iter().filter(|c| c.available(true)) {
                    self.print(ChatKind::System, format!("{} - {}", c.usage(), c.help));
                }
                return;
            }
            "respawn" | "kill" => {
                self.runner.send(Command::Destroy);
                Ok("Destroyed your ship".into())
            }
            "fuel" => {
                self.runner.send(Command::Refuel);
                Ok("Refilled your delta-v".into())
            }
            "god" => {
                let on = !self.runner.published.lock().unwrap().god;
                self.runner.send(Command::God(on));
                Ok(if on { "God mode on" } else { "God mode off" }.into())
            }
            "speed" => args.first().and_then(|a| command::parse_span(a)).ok_or_else(String::new).map(|pace| {
                self.set_pace(pace);
                if pace > 0.0 { format!("One second is now {}", fmt::span(pace)) } else { "Time stands still".into() }
            }),
            "accuracy" => args.first().and_then(|a| a.parse::<f32>().ok()).filter(|v| (0.2..=1.5).contains(v)).ok_or_else(String::new).map(|v| {
                self.runner.send(Command::Theta(v));
                format!("Opening angle set to {v}")
            }),
            // Without a name, the selected body.
            "orbit" => match args.first() {
                Some(word) => self.body(word),
                None => selected.and(self.selected).ok_or_else(String::new),
            }
            .map(|id| {
                self.runner.send(Command::Orbit(id));
                format!("Put your ship on an orbit around B{id}")
            }),
            "tp" => self.teleport(&args).ok_or_else(String::new),
            _ => Err(String::new()),
        };
        match result {
            Ok(text) => self.print(ChatKind::System, text),
            Err(text) if text.is_empty() => self.print(ChatKind::Error, format!("usage: {}", spec.usage())),
            Err(text) => self.print(ChatKind::Error, text),
        }
    }

    /// `B123` to the id of a body that still exists.
    fn body(&self, word: &str) -> Result<u32, String> {
        let id = word.strip_prefix(['B', 'b']).and_then(|n| n.parse::<u32>().ok()).filter(|id| self.runner.bodies.read().unwrap().locate(*id).is_some());
        id.ok_or_else(|| format!("no body called {word:?}"))
    }

    /// `/tp <x> <y>`, each in metres and optionally relative to the ship (`~`).
    fn teleport(&mut self, args: &[&String]) -> Option<String> {
        let ship = self.last_ship.unwrap_or_default();
        let [x, y] = args else { return None };
        let coordinate = |word: &str, base: f64| match word.strip_prefix('~') {
            Some("") => Some(base),
            Some(offset) => command::parse_metres(offset).map(|d| base + d),
            None => command::parse_metres(word),
        };
        let (x, y) = coordinate(x, ship.x).zip(coordinate(y, ship.y))?;
        self.runner.send(Command::Place(Particle { x, y, ..ship }));
        Some(format!("Teleported you to {}, {}", fmt::distance(x), fmt::distance(y)))
    }

    pub fn frame(&mut self, settings: &mut Settings) -> Outcome {
        // To notice what this frame changes, by key or in the menu, and save it.
        let before = settings.clone();
        let dt = get_frame_time().min(0.1);
        let hud = settings.ui_factor();
        let ui = settings.marker_factor();
        let keys = !self.ui_has_keyboard && !self.menu_open && !self.chat.open;
        let mouse = mouse_position();

        let (seen, events, merges) = {
            let mut p = self.runner.published.lock().unwrap();
            let seen = Seen {
                clock: p.clock,
                ship: p.ship,
                ship_before: p.ship_before,
                ship_tick: p.ship_tick,
                respawn_tick: p.respawn_tick,
                tick: p.tick,
                god: p.god,
                pace: p.pace,
                ship_dt: p.ship_dt,
                time: p.time,
                theta: p.theta,
                stats: p.stats.clone(),
                heaviest: p.heaviest.clone(),
                local: p.local.clone(),
                watch: p.watch.clone(),
            };
            (seen, std::mem::take(&mut p.events), std::mem::take(&mut p.merges))
        };
        self.flashes.extend(merges);
        for e in events {
            if let Event::Crashed { x, y, vx, vy, .. } = e {
                self.wrecks.push((x, y, vx, vy, seen.ship_tick));
            }
        }

        // --- toggles -------------------------------------------------------------------------
        if is_key_pressed(KeyCode::Escape) {
            if self.chat.open {
                self.chat.escape();
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
            settings.save();
        }
        if keys {
            if is_key_pressed(KeyCode::Space) {
                self.set_pace(if seen.pace > 0.0 { 0.0 } else { self.last_pace });
            }
            // < and > (comma and period): held, they change the pace smoothly.
            let turn = is_key_down(KeyCode::Period) as i32 - is_key_down(KeyCode::Comma) as i32;
            if turn != 0 {
                self.set_pace(self.last_pace * 2f64.powf(turn as f64 * SPEED_KEY_RATE * dt as f64));
            }
            if is_key_pressed(KeyCode::P) {
                settings.prediction = !settings.prediction;
            }
            if is_key_pressed(KeyCode::C) {
                let at = ColorMode::ALL.iter().position(|m| *m == settings.color).unwrap_or(0);
                settings.color = ColorMode::ALL[(at + 1) % ColorMode::ALL.len()];
                self.say(&format!("Colour shows {}", settings.color.name()));
            }
            if is_key_pressed(KeyCode::T) || is_key_pressed(KeyCode::Enter) {
                self.chat.open_with("");
            } else if is_key_pressed(KeyCode::Slash) {
                self.chat.open_with("/");
            }
            if is_key_pressed(KeyCode::F) {
                settings.follow_selection = !settings.follow_selection;
                self.say(if settings.follow_selection { "Following selection" } else { "Following own ship" });
            }
            if is_key_pressed(KeyCode::M) {
                settings.mouse_aim = !settings.mouse_aim;
                settings.save();
                self.say(if settings.mouse_aim { "Aim: mouse" } else { "Aim: A/D keys" });
            }
            if is_key_pressed(KeyCode::X) {
                self.thrust_pct = 0.0;
            }
            if is_key_pressed(KeyCode::Z) {
                self.thrust_pct = 100.0;
            }
            if is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift) {
                self.thrust_pct = (self.thrust_pct + 80.0 * dt).min(100.0);
            } else if is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl) {
                self.thrust_pct = (self.thrust_pct - 80.0 * dt).max(0.0);
            }
        }

        // --- controls (from last frame's view of the world) -----------------------------------
        let (mwx, mwy) = self.view.to_world(mouse.0, mouse.1);
        let mut input = ShipInput::default();
        if let Some(ship) = self.last_ship {
            if settings.mouse_aim {
                self.heading = (mwy - ship.y).atan2(mwx - ship.x);
            } else if keys {
                self.heading += TURN_RATE * dt as f64 * (is_key_down(KeyCode::D) as i32 - is_key_down(KeyCode::A) as i32) as f64;
            }
            input.angle = math::dir_to_angle(self.heading.cos(), self.heading.sin());
            if keys && (is_key_down(KeyCode::W) || is_key_down(KeyCode::Up)) {
                input.thrust = self.thrust_pct.round() as u8;
            }
        }
        self.runner.send(Command::Input(input));

        // A watched body that merged lives on in its survivor.
        if seen.tick > self.watch_told + 1 && self.selected.is_some() && seen.watch.first().is_some_and(|w| Some(*w) != self.selected) {
            self.selected = seen.watch.first().copied();
        }

        clear_background(style::BACKGROUND);
        let pointer = !self.ui_has_pointer && !self.menu_open;
        let wheel = mouse_wheel().1;
        let clicked = is_mouse_button_released(MouseButton::Left) && self.drag_from.is_some() && self.drag_moved <= 10.0 * ui && pointer;
        let mut pick: Option<Option<u32>> = None;
        let (big, present, ship, picked, bodies_n, flashes_now);
        {
            let b = self.runner.bodies.read().unwrap();
            bodies_n = b.len();
            present = (seen.clock.tick + seen.clock.at.elapsed().as_secs_f64() * seen.clock.rate).min(b.tick as f64);
            // Seconds from where the arrays are to the moment being drawn (rarely positive).
            let tau = (present - b.tick as f64) * b.dt;
            let at = |i: usize| Particle { x: b.x[i] + b.vx[i] * tau, y: b.y[i] + b.vy[i] * tau, vx: b.vx[i], vy: b.vy[i] };
            let mut chosen = self.selected.and_then(|id| b.locate(id));
            if chosen.is_none() {
                self.selected = None;
            }
            ship = seen.ship.map(|s1| {
                let s0 = seen.ship_before.unwrap_or(s1).p;
                let (s1, h) = (s1.p, seen.ship_dt);
                // Hermite between the two ticks the simulation has for the ship.
                let u = (present - (seen.ship_tick as f64 - 1.0)).clamp(0.0, 1.0);
                let (u2, u3) = (u * u, u * u * u);
                let (a, bb, c, d) = (2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, -2.0 * u3 + 3.0 * u2, u3 - u2);
                Particle {
                    x: a * s0.x + bb * h * s0.vx + c * s1.x + d * h * s1.vx,
                    y: a * s0.y + bb * h * s0.vy + c * s1.y + d * h * s1.vy,
                    vx: s0.vx + (s1.vx - s0.vx) * u,
                    vy: s0.vy + (s1.vy - s0.vy) * u,
                }
            });

            // --- camera ------------------------------------------------------------------------
            let target = match (settings.follow_selection, ship, self.selected) {
                (true, _, Some(s)) => Target::Body(s),
                (_, Some(_), _) => Target::Ship,
                _ => Target::Free,
            };
            let target_pos = match target {
                Target::Ship => ship.map(|p| (p.x, p.y)).unwrap(),
                Target::Body(_) => chosen.map(|i| (at(i).x, at(i).y)).unwrap(),
                Target::Free => self.last_target_pos,
            };
            if !self.view_ready {
                let (x0, x1) = b.x.par_iter().fold(|| (f64::MAX, f64::MIN), |a, v| (a.0.min(*v), a.1.max(*v))).reduce(|| (f64::MAX, f64::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
                self.view.mpp = ((x1 - x0).max(1.0e9) * 0.75 / screen_height().min(screen_width()) as f64).max(1.0);
                self.view_ready = true;
            } else if target != self.last_target {
                self.offset = if target == Target::Ship && self.last_target == Target::Free { (0.0, 0.0) } else { (self.view.cx - target_pos.0, self.view.cy - target_pos.1) };
            }
            if let (false, Some(ship)) = (self.zoomed_for_ship, ship) {
                // First sight of our ship: frame the world out to the ship's own orbit.
                self.zoomed_for_ship = true;
                let (m, mx, my) = (0..b.len()).into_par_iter().map(|i| (b.m[i] as f64, b.m[i] as f64 * b.x[i], b.m[i] as f64 * b.y[i])).reduce(|| (0.0, 0.0, 0.0), |p, q| (p.0 + q.0, p.1 + q.1, p.2 + q.2));
                if m > 0.0 {
                    let d = ((ship.x - mx / m).powi(2) + (ship.y - my / m).powi(2)).sqrt();
                    self.view.mpp = (d * 2.6 / screen_height().min(screen_width()) as f64).max(1.0);
                }
            }
            self.last_target = target;
            self.last_target_pos = target_pos;
            if let (Target::Body(_), Some(ship), false) = (target, ship, self.had_ship) {
                frame_both(&mut self.view, &mut self.offset, target_pos, (ship.x, ship.y));
            }
            self.had_ship = ship.is_some();
            if pointer && !self.chat.open && wheel != 0.0 {
                self.zoom_pending += (wheel * settings.zoom_speed).clamp(-1.5, 1.5);
            }
            if self.zoom_pending.abs() > 1e-3 {
                let step = self.zoom_pending * (12.0 * dt).min(1.0);
                self.zoom_pending -= step;
                self.view.cx = target_pos.0 + self.offset.0;
                self.view.cy = target_pos.1 + self.offset.1;
                let before = self.view.to_world(mouse.0, mouse.1);
                self.view.mpp = (self.view.mpp * 1.1f64.powf(-step as f64)).clamp(0.05, 1.0e12);
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
            if !dragging {
                self.drag_from = None;
            }
            self.last_mouse = mouse;
            self.view.cx = target_pos.0 + self.offset.0;
            self.view.cy = target_pos.1 + self.offset.1;
            let view = self.view;

            if clicked {
                // The heaviest body right under the cursor, else the nearest one around it.
                let (mx, my) = view.to_world(mouse.0, mouse.1);
                let reach = (PICK_RADIUS_PX * ui) as f64 * view.mpp;
                let close = 0.4 * reach;
                let best = (0..b.len())
                    .into_par_iter()
                    .filter_map(|i| {
                        let d = ((b.x[i] + b.vx[i] * tau - mx).powi(2) + (b.y[i] + b.vy[i] * tau - my).powi(2)).sqrt() - b.r[i] as f64;
                        (b.m[i] > 0.0 && d <= reach).then_some((i, d, b.m[i]))
                    })
                    .reduce_with(|p, q| match (p.1 <= close, q.1 <= close) {
                        (true, true) => if q.2 > p.2 { q } else { p },
                        (true, false) => p,
                        (false, true) => q,
                        (false, false) => if q.1 < p.1 { q } else { p },
                    });
                pick = Some(best.map(|c| b.id[c.0]));
                chosen = best.map(|c| c.0);
            }

            // --- the world ---------------------------------------------------------------------
            self.flashes.retain(|(_, tick)| present - (*tick as f64) < FLASH_TICKS);
            let flashes: Vec<Flash> = self
                .flashes
                .iter()
                .map(|(m, tick)| {
                    let age = present - *tick as f64;
                    Flash { x: m.x + m.vx * age * b.dt, y: m.y + m.vy * age * b.dt, life: (1.0 - age / FLASH_TICKS).clamp(0.0, 1.0) as f32, mass: m.mass }
                })
                .collect();
            big = self.density.project(&b, &view, tau, settings.color);
            flashes_now = flashes;
            picked = chosen.map(|i| Picked { at: at(i), mass: b.m[i] as f64, radius: b.r[i] as f64 });
        }
        // The bodies are free again: the rest of the picture does not need them.
        self.density.present(&flashes_now);
        if let Some(id) = pick {
            self.select(id, seen.tick);
        }
        self.last_ship = ship;
        let view = self.view;
        let mut labels = style::Labels::default();
        self.draw_bodies(&big, ui);

        // --- wrecks ----------------------------------------------------------------------------
        self.wrecks.retain(|w| present - (w.4 as f64) < 90.0);
        for (x, y, vx, vy, tick) in &self.wrecks {
            let age = ((present - *tick as f64) / 60.0).max(0.0);
            let s = view.to_screen(x + vx * age * seen.pace, y + vy * age * seen.pace);
            let fade = (1.0 - age / 1.5).clamp(0.0, 1.0) as f32;
            style::ring(s.0, s.1, (10.0 + 60.0 * age as f32) * ui, 3.0 * ui, Color::new(1.0, 0.9, 0.5, fade));
        }

        // --- prediction ------------------------------------------------------------------------
        self.look_ahead(&seen);
        let ref_slot = self.ahead.as_ref().zip(self.selected).and_then(|(a, id)| a.world.ids.iter().position(|v| *v == id)).map(|s| s as u32);
        if self.predictor.poll() {
            match (seen.ship, &self.ahead) {
                (Some(state), Some(ahead)) if settings.prediction && ahead.eph.reader().is_some() => {
                    let mut timeline = InputTimeline::new();
                    timeline.set(seen.ship_tick, input);
                    self.predictor.submit(Job {
                        reader: ahead.eph.reader().unwrap(),
                        rules: self.rules_now(),
                        ship: state,
                        start: seen.ship_tick,
                        timeline,
                        ticks: self.lookahead_ticks,
                        show_ship: true,
                        held: input.thrust > 0,
                        shell: None,
                        ref_slot,
                    });
                }
                _ => self.predictor.clear(),
            }
        }
        let paths = &self.predictor.latest;
        // Relative paths are only meaningful for the body they were computed against.
        let anchor = match (ref_slot, picked) {
            (Some(_), Some(p)) => (p.at.x, p.at.y),
            _ => (0.0, 0.0),
        };
        if paths.ref_slot == ref_slot {
            draw_path(&paths.held, anchor, &view, ui, Color::from_rgba(255, 220, 90, 150));
            draw_path(&paths.coast, anchor, &view, ui, Color::from_rgba(90, 255, 120, 170));
            if let (true, Some(q)) = (paths.coast_impact, paths.coast.last()) {
                let s = view.to_screen(anchor.0 + q.0, anchor.1 + q.1);
                draw_line(s.0 - 6.0 * ui, s.1 - 6.0 * ui, s.0 + 6.0 * ui, s.1 + 6.0 * ui, 2.0 * ui, RED);
                draw_line(s.0 - 6.0 * ui, s.1 + 6.0 * ui, s.0 + 6.0 * ui, s.1 - 6.0 * ui, 2.0 * ui, RED);
                labels.push("impact", s.0, s.1 + 20.0 * ui, LABEL * ui, RED, Rank::Approach);
            } else if let Some((i, d)) = paths.closest.filter(|(i, _)| *i > 0 && i + 1 < paths.coast.len()) {
                let q = paths.coast[i];
                let s = view.to_screen(anchor.0 + q.0, anchor.1 + q.1);
                style::ring(s.0, s.1, 5.0 * ui, 1.5 * ui, Color::from_rgba(90, 255, 120, 255));
                let lines = vec![format!("closest {}", fmt::distance(d)), format!("in {:.1} s", i as f64 / 60.0)];
                labels.block(lines, s.0, s.1 + 18.0 * ui, LABEL * ui, Color::from_rgba(150, 255, 170, 255), Rank::Approach);
            }
        }

        // --- ship ------------------------------------------------------------------------------
        if self.wake_ref != self.selected {
            self.wake.clear();
            self.wake_ref = self.selected;
        }
        let frame_of = picked.map_or((0.0, 0.0), |p| (p.at.x, p.at.y));
        match ship {
            Some(p) => {
                if seen.ship_tick >= self.wake_tick + 2 {
                    self.wake_tick = seen.ship_tick;
                    self.wake.push_back((p.x - frame_of.0, p.y - frame_of.1));
                    while self.wake.len() > WAKE_POINTS {
                        self.wake.pop_front();
                    }
                }
                let mut prev: Option<(f32, f32)> = None;
                for (k, q) in self.wake.iter().enumerate() {
                    let s = view.to_screen(q.0 + frame_of.0, q.1 + frame_of.1);
                    if let Some(a) = prev.filter(|a| (a.0 - s.0).abs() + (a.1 - s.1).abs() < 400.0) {
                        let f = k as f32 / WAKE_POINTS as f32;
                        draw_line(a.0, a.1, s.0, s.1, 1.5 * ui, style::alpha(style::OWN_SHIP, 0.5 * f * f));
                    }
                    prev = Some(s);
                }
                let s = view.to_screen(p.x, p.y);
                let fuel = seen.ship.map_or(0, |s| s.fuel);
                draw_ship(s, self.heading as f32, style::OWN_SHIP, input.thrust > 0 && fuel > 0 && seen.pace > 0.0, ui);
            }
            None => self.wake.clear(),
        }

        // --- selection -------------------------------------------------------------------------
        if let (Some(id), Some(p)) = (self.selected, picked) {
            let s = view.to_screen(p.at.x, p.at.y);
            let r_px = ((p.radius / view.mpp) as f32).min(4000.0) + 6.0 * ui;
            draw_rectangle_lines(s.0 - r_px, s.1 - r_px, 2.0 * r_px, 2.0 * r_px, 1.5 * ui, WHITE);
            let mut lines = vec![format!("B{id}"), format!("m = {}", fmt::mass(p.mass)), format!("r = {}", fmt::distance(p.radius))];
            if let Some(ship) = ship {
                let d = ((p.at.x - ship.x).powi(2) + (p.at.y - ship.y).powi(2)).sqrt();
                let v = ((p.at.vx - ship.vx).powi(2) + (p.at.vy - ship.vy).powi(2)).sqrt();
                lines.push(format!("d = {}", fmt::distance(d)));
                lines.push(format!("rel v = {}", fmt::speed(v)));
            }
            labels.block(lines, s.0, s.1 + r_px + 12.0 * ui, LABEL * ui, WHITE, Rank::Selection);
        }
        labels.draw();
        style::scale_bar(screen_width() * 0.5, screen_height() - 18.0 * hud, view.mpp, fmt::distance_round, hud);

        self.hud(settings, &before, &seen, bodies_n, present, picked, wheel, hud)
    }

    fn draw_bodies(&self, big: &[Big], ui: f32) {
        for b in big {
            style::body((b.x, b.y), b.r, b.mass as f64, b.id, 1.0, ui);
        }
    }

    /// Keep a small exact simulation of the ship's surroundings running ahead of the present.
    fn look_ahead(&mut self, seen: &Seen) {
        let done = |a: &Ahead| a.eph.end_tick().is_some_and(|end| end + 30 >= a.upto);
        // A finished replacement takes over; until then the old one keeps the line steady.
        if self.next_ahead.as_ref().is_some_and(done) {
            self.ahead = self.next_ahead.take();
        }
        if self.ahead.as_ref().is_some_and(|a| a.pace != self.last_pace) || self.next_ahead.as_ref().is_some_and(|a| a.pace != self.last_pace) {
            (self.ahead, self.next_ahead) = (None, None);
        }
        // While the pace is still being changed, anything started now would be thrown away.
        if self.pace_changed.elapsed().as_secs_f64() < 0.25 {
            return;
        }
        let Some(local) = &seen.local else { return };
        if seen.ship.is_none() {
            (self.ahead, self.next_ahead) = (None, None);
            return;
        }
        let newest = self.next_ahead.as_ref().or(self.ahead.as_ref()).map(|a| a.world.snapshot.tick);
        if self.next_ahead.is_none() && newest.is_none_or(|t| local.snapshot.tick >= t + 20) && !local.snapshot.x.is_empty() {
            let mut eph = Eph::new(&local.snapshot, &self.rules_now(), true);
            let upto = local.snapshot.tick + self.lookahead_ticks as u64 + 90;
            eph.request(upto, 0);
            let ahead = Ahead { eph, world: local.clone(), upto, pace: self.last_pace };
            if self.ahead.is_none() {
                self.ahead = Some(ahead);
            } else {
                self.next_ahead = Some(ahead);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn hud(&mut self, settings: &mut Settings, before: &Settings, seen: &Seen, bodies: usize, present: f64, picked: Option<Picked>, wheel: f32, hud: f32) -> Outcome {
        self.meter.frame(self.density.last_ms);
        let stats = &seen.stats;
        let title = format!("{}   ·   {} BODIES", self.scenario.to_uppercase(), group_digits(bodies));
        // What one real second is worth, as achieved; and as asked for when that is more.
        let pace = match seen.pace {
            s if s <= 0.0 => "PAUSED".to_string(),
            s if stats.rate + 1.0 < stats.wanted_rate => format!("1 s = {}  (of {})", fmt::span(s * (stats.rate / stats.wanted_rate) as f64), fmt::span(s)),
            s => format!("1 s = {}", fmt::span(s)),
        };
        let days = seen.time / 86_400.0;
        let stat_lines = [
            format!("{} bodies   {:.0} fps   draw {:.1} ms", group_digits(bodies), self.meter.fps, self.meter.ms),
            format!("step {:.1} ms  (sort {:.1}  gravity {:.1}  merge {:.1})", stats.step_ms, stats.sort_ms, stats.force_ms, stats.finish_ms),
            format!("{:.0} of {:.0} ticks per second   day {days:.0}", stats.rate, stats.wanted_rate),
            format!("{:.0} pulls per body   angle {:.2}   {}", stats.interactions, seen.theta, stats.level),
            match stats.error {
                Some(e) => format!("force error {:.3} %", e * 100.0),
                None => "force error: measuring".to_string(),
            },
            format!("merges {} ({:.0}/s)   escaped {}", group_digits(stats.merges_total as usize), stats.merges_per_s, stats.removed_total),
            format!("ship feels {} bodies and lumps   prediction {:.0} ms", stats.local, self.predictor.latest.compute_ms),
        ];
        let status = self.status.as_ref().filter(|s| s.1 > get_time()).map(|s| s.0.clone());
        let fuel = seen.ship.map(|s| s.fuel);
        let respawn_in = seen.respawn_tick.map(|t| (t as f64 - present).max(0.0) / 60.0);
        let fuel_max = self.rules.fuel_max_mmps;
        let (thrust_pct, god, speed, theta) = (self.thrust_pct, seen.god, seen.pace, seen.theta);
        let player = vec![self.player.clone()];
        let mut mentions: Vec<(String, Mention)> = seen.heaviest.iter().map(|h| (format!("B{}", h.id), Mention::Body(h.id))).collect();
        if let Some(id) = self.selected.filter(|id| !seen.heaviest.iter().any(|h| h.id == *id)) {
            mentions.push((format!("B{id}"), Mention::Body(id)));
        }
        let body_names: Vec<String> = mentions.iter().map(|m| m.0.clone()).collect();
        let complete_ctx = Context { players: &player, bodies: &body_names, presets: &[], op: true, sandbox: true };
        let now = self.now();

        let mut outcome = Outcome::Continue;
        let mut menu = self.menu_open;
        let (mut has_ptr, mut has_kb) = (false, false);
        let mut chat_out = chat::Outcome::default();
        let mut options = settings.clone();
        let (mut new_speed, mut new_theta, mut new_god, mut new_pick) = (None, theta, god, None);
        egui_macroquad::ui(|ctx| {
            use egui::{Align2, Area, Id, RichText};
            ctx.set_zoom_factor(hud);
            let (gold, dim, bad) = (style::c32(style::GOLD), style::c32(style::DIM), style::c32(style::EMBER));

            Area::new(Id::new("title")).anchor(Align2::CENTER_TOP, [0.0, 10.0]).interactable(false).show(ctx, |ui| {
                // One line each, however long: wrapped, the title looks broken.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(&title).small().color(dim).extra_letter_spacing(2.0));
                    if !pace.is_empty() {
                        ui.label(RichText::new(&pace).heading().color(if speed > 0.0 { style::c32(style::TEXT) } else { gold }));
                    }
                    if let Some(s) = &status {
                        ui.label(RichText::new(s).color(gold));
                    }
                });
            });

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
                            let mine = self.selected == Some(h.id);
                            let colour = if mine { gold } else { style::c32(style::mass_color(h.mass as f64)) };
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

            Area::new(Id::new("ship")).anchor(Align2::RIGHT_BOTTOM, [-10.0, -10.0]).interactable(false).show(ctx, |ui| {
                style::panel().show(ui, |ui| {
                    ui.set_width(220.0);
                    match fuel {
                        Some(f) => {
                            let frac = f as f32 / fuel_max as f32;
                            let value = if god { "endless".to_string() } else { fmt::speed(f as f64 / 1000.0) };
                            style::gauge(ui, "DELTA-V", &value, frac, if frac < 0.2 { style::EMBER } else { style::ACCENT });
                            style::gauge(ui, "THROTTLE", &format!("{thrust_pct:.0} %"), thrust_pct / 100.0, style::GOOD);
                            if god {
                                style::caption(ui, "INDESTRUCTIBLE");
                            }
                        }
                        None => {
                            style::caption(ui, "SHIP LOST");
                            let text = match respawn_in {
                                Some(s) => format!("respawn in {s:.1} s"),
                                None => "waiting for a ship".into(),
                            };
                            ui.label(RichText::new(text).heading().color(bad));
                        }
                    }
                });
            });

            chat_out = self.chat.show(ctx, &self.log, now, wheel * settings.zoom_speed, &complete_ctx, &mentions);
            if menu {
                egui::Window::new("MENU").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                    ui.set_width(330.0);
                    crate::options::scrolled(ui, |ui| {
                        style::section(ui, "WORLD");
                        ui.add(egui::Slider::new(&mut new_theta, 0.3..=1.2).text("opening angle")).on_hover_text("Smaller is more accurate and slower");
                        ui.checkbox(&mut new_god, "Indestructible ship with endless fuel");
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

        if let Some(text) = chat_out.send {
            self.command(&text, picked);
        }
        if let Some(Mention::Body(id)) = chat_out.clicked {
            new_pick = Some(id);
        }
        if let Some(id) = new_pick {
            self.select(Some(id), seen.tick);
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
            self.print(ChatKind::Error, format!("Unable to draw on the graphics card: {why}"));
            self.use_gpu(settings, false);
        }
        if new_god != god {
            self.runner.send(Command::God(new_god));
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
