//! In-game screen: input, camera, rendering and HUD. All game logic lives in
//! `gsim-client-core`; this file only looks at the replica and draws it.

use crate::chat::{self, ChatBox, Mention};
use crate::density::{ColorMode, Density, Flash};
use crate::fmt;
use crate::style::{self, Rank};
use crate::predictor::{Job, Predictor, RefKey, Reference};
use crate::settings::{Camera, Settings};
use egui_macroquad::egui;
use gsim_client_core::complete::Context;
use gsim_client_core::net::NetClient;
use gsim_server::scenario::{Params, PRESETS};
use gsim_client_core::world::World;
use gsim_client_core::{Controls, EffectKind, SessionConfig};
use gsim_core::objective::orbit_status;
use gsim_core::{math, EphRow, Particle, SystemFrame, Tick};
use gsim_proto::shop::{self, Upgrade, PICKUP_RADIUS, TIERS};
use gsim_proto::{GameMode, Items, TeamId, Twist, Objective, PieceKind, PlayerId, Purse, Supply, Upgrades, Ware, BASE_HP, mine_seconds};
#[cfg(not(target_arch = "wasm32"))]
use gsim_server::net::{build_authority, run, ServerOptions};
use macroquad::prelude::*;
use std::collections::VecDeque;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::{
    net::{SocketAddr, UdpSocket},
    sync::atomic::{AtomicBool, Ordering},
    thread::JoinHandle,
};

/// In-process server for solo play.
#[cfg(not(target_arch = "wasm32"))]
pub struct Solo {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// (In a browser there is none: nobody can be handed one.)
#[cfg(target_arch = "wasm32")]
pub enum Solo {}

#[cfg(not(target_arch = "wasm32"))]
impl Solo {
    pub fn start(preset: &str, seed: u64, params: &Params) -> Result<(Self, SocketAddr), String> {
        // Let the OS pick a free port, then hand it to the server.
        let port = UdpSocket::bind("127.0.0.1:0").and_then(|s| s.local_addr()).map_err(|e| e.to_string())?.port();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        // Whoever hosts a solo game may use every command.
        let opts = ServerOptions { bind: addr, preset: preset.to_string(), seed, params: params.clone(), quiet: true, max_clients: 4, op_all: true, ..Default::default() };
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

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Solo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Camera: world meters (f64) to screen pixels, relative to a double-precision center so
/// nothing loses precision at astronomical distances.
#[derive(Clone, Copy)]
pub(crate) struct View {
    pub cx: f64,
    pub cy: f64,
    /// Metres per pixel.
    pub mpp: f64,
}

impl View {
    pub fn to_screen(self, x: f64, y: f64) -> (f32, f32) {
        (
            ((x - self.cx) / self.mpp) as f32 + screen_width() * 0.5,
            ((y - self.cy) / self.mpp) as f32 + screen_height() * 0.5,
        )
    }

    pub fn to_world(self, sx: f32, sy: f32) -> (f64, f64) {
        (
            self.cx + (sx - screen_width() * 0.5) as f64 * self.mpp,
            self.cy + (sy - screen_height() * 0.5) as f64 * self.mpp,
        )
    }

    pub fn on_screen(&self, s: (f32, f32), margin: f32) -> bool {
        s.0 > -margin && s.1 > -margin && s.0 < screen_width() + margin && s.1 < screen_height() + margin
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Ship,
    /// Another player'"'"'s ship (picked by clicking their name in chat).
    Player(PlayerId),
    Body(u32),
    /// A piece or another ship that was picked.
    Picked(RefKey),
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
    /// How long the display gives a frame (the shortest seen lately), and how much of that
    /// a frame takes without its predictions (s).
    period: f64,
    busy: f64,
    /// Predictions finished a second, and when and at how many that was last worked out.
    predicting: (f64, f64, u64),
    _solo: Option<Solo>,
    view: View,
    settings_dirty: bool,
    /// The ship color the server knows about.
    color_sent: [u8; 3],
    /// Since when the self-destruct key has been held (`None` again once it has fired,
    /// until the key is let go).
    destruct: Option<f64>,
    destruct_done: bool,
    view_ready: bool,
    zoomed_for_ship: bool,
    had_ship: bool,
    zoom_pending: f32,
    /// Camera offset from whatever it follows.
    offset: (f64, f64),
    auto: Auto,
    /// R was pressed: select a target.
    pick_target: bool,
    chat: ChatBox,
    /// Camera follows this player (set by clicking their name in chat).
    watch: Option<PlayerId>,
    last_target: Target,
    last_target_pos: (f64, f64),
    selected: Option<u32>,
    /// A piece or (with a target lock) another ship picked instead of a body, to fly to or aim at.
    picked: Option<RefKey>,
    heading: f64,
    thrust_pct: f32,
    menu_open: bool,
    ui_has_pointer: bool,
    ui_has_keyboard: bool,
    drag_from: Option<(f32, f32)>,
    drag_moved: f32,
    last_mouse: (f32, f32),
    frames: VecDeque<TrailFrame>,
    last_frame_tick: Tick,
    predictor: Predictor,
    meter: fmt::Meter,
    lookahead_ticks: u32,
    density: Density,
    /// Recent merges: position and velocity of the survivor, mass absorbed, tick.
    flashes: Vec<(f64, f64, f64, f64, f32, Tick)>,
    flash_tick: Tick,
    /// The round the selection, trails and camera belong to.
    round: Option<u32>,
    status: Option<(String, f64)>,
    started: f64,
    /// The shop is showing (at home, in a round of bases).
    shop_open: bool,
}

const TRAIL_EVERY_TICKS: Tick = 4;
const TRAIL_FRAMES: usize = 150;
pub(crate) const PICK_RADIUS_PX: f32 = 26.0;
pub(crate) const TURN_RATE: f64 = 2.85;
/// Font size of labels drawn in the world, before marker scaling.
pub(crate) const LABEL: f32 = 12.0;
/// The closest the view zooms in, in meters per pixel.
pub(crate) const MIN_MPP: f64 = 1.0e-6;
/// What the wheel's movement is multiplied by before the zoom speed setting. A browser
/// reports it in pixels, a hundred or so to a notch (as macOS does, whose default zoom
/// speed allows for that); elsewhere a notch is one.
const WHEEL: f32 = if cfg!(target_arch = "wasm32") { 0.02 } else { 1.0 };
/// Seconds of a frame kept clear of predictions, for what goes on after this screen is done
/// with it: the window library sending the picture off, and the browser's own work.
const SLACK: f64 = 0.0015;
/// The scales (m per pixel) refocusing on the ship keeps the view within: close enough to see
/// the ship's surroundings, far enough to see its orbit.
const FOCUS_MPP: (f64, f64) = (2.0e5, 5.0e6);
/// Seconds the self-destruct key has to be held.
const DESTRUCT_HOLD: f64 = 1.5;
/// How long news for the player stays under the clock (s).
const ALERT_SECONDS: f64 = 4.0;
/// Keys that buy upgrades in the shop: those for the ship, then those for the base.
const SHOP_KEYS: [KeyCode; 9] = [KeyCode::Key1, KeyCode::Key2, KeyCode::Key3, KeyCode::Key4, KeyCode::Key5, KeyCode::Key6, KeyCode::Key7, KeyCode::Key8, KeyCode::Key9];

impl Game {
    /// `server`: host or host:port (in a browser, nothing: the server the page came from).
    pub fn connect(server: &str, settings: &Settings, solo: Option<Solo>, lookahead_seconds: f32) -> Result<Self, String> {
        let cfg = SessionConfig {
            name: settings.name.clone(),
            color: settings.ship_color,
            identity: settings.identity(),
            // On a thread of its own; in a browser, in the worker beside the page.
            #[cfg(not(target_arch = "wasm32"))]
            eph: gsim_client_core::eph::Source::Thread,
            #[cfg(target_arch = "wasm32")]
            eph: gsim_client_core::eph::Source::Fed(crate::worker::feed),
            // Tick rate is only known after joining; 60 Hz is the server default.
            lookahead_ticks: (lookahead_seconds.clamp(2.0, 60.0) * 60.0) as u32,
        };
        Ok(Self {
            net: NetClient::connect(server, cfg)?,
            _solo: solo,
            view: View { cx: 0.0, cy: 0.0, mpp: 1.0e9 },
            settings_dirty: false,
            color_sent: settings.ship_color,
            destruct: None,
            destruct_done: false,
            view_ready: false,
            zoomed_for_ship: false,
            had_ship: false,
            zoom_pending: 0.0,
            auto: Auto { moving: false, small_since: None, path: None, path_at: 0.0, hands_off_until: 0.0, framed: None },
            pick_target: false,
            offset: (0.0, 0.0),
            chat: ChatBox::default(),
            watch: None,
            last_target: Target::Free,
            last_target_pos: (0.0, 0.0),
            picked: None,
            selected: None,
            heading: 0.0,
            thrust_pct: 100.0,
            menu_open: false,
            ui_has_pointer: false,
            ui_has_keyboard: false,
            drag_from: None,
            drag_moved: 0.0,
            last_mouse: mouse_position(),
            frames: VecDeque::new(),
            last_frame_tick: 0,
            period: 0.0,
            busy: 0.0,
            predicting: (0.0, 0.0, 0),
            predictor: Predictor::new(),
            meter: fmt::Meter::default(),
            lookahead_ticks: (lookahead_seconds.clamp(2.0, 60.0) * 60.0) as u32,
            density: {
                let mut density = Density::new();
                density.crowd = 0.0;
                density
            },
            flashes: Vec::new(),
            flash_tick: 0,
            round: None,
            status: None,
            started: get_time(),
            shop_open: false,
        })
    }

    fn say(&mut self, text: &str) {
        self.status = Some((text.to_string(), get_time() + 2.0));
    }

    pub fn frame(&mut self, settings: &mut Settings) -> Outcome {
        let began = fmt::now();
        // (Allowed to creep up, so that it follows a display that has become slower.)
        let took = get_frame_time() as f64;
        self.period = if self.period > 0.0 { (1.002 * self.period).min(took) } else { took };
        // To notice what this frame changes, by key or in the menu, and save it.
        let before = settings.clone();
        self.meter.frame(self.density.last_ms);
        let dt = get_frame_time().min(0.1);
        // HUD panels scale with the window; things drawn in the world (ship, dots, labels) only mildly.
        let hud = settings.ui_factor();
        let ui = settings.marker_factor();
        let keys = !self.ui_has_keyboard && !self.menu_open && !self.chat.open;
        let mouse = mouse_position();
        let mut shop_asked = false;
        // V was pressed: back to our ship (and the selection).
        let mut refocus = false;
        // E was pressed: fire a gravity well towards the cursor, as a shell.
        let mut well_asked = false;

        // --- toggles -------------------------------------------------------------------------
        if is_key_pressed(KeyCode::Escape) {
            // Escape hides the suggestion list, then closes the chat line; then the shop; otherwise
            // the menu.
            if self.chat.open {
                self.chat.escape();
            } else if self.shop_open {
                self.shop_open = false;
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
            if is_key_pressed(KeyCode::L) {
                settings.trails = !settings.trails;
                self.say(if settings.trails { "Trails on" } else { "Trails off" });
            }
            if is_key_pressed(KeyCode::K) {
                settings.trails_relative = !settings.trails_relative;
                self.say(if settings.trails_relative { "Trails relative to reference" } else { "Trails in world frame" });
            }
            if is_key_pressed(KeyCode::P) {
                settings.prediction = !settings.prediction;
            }
            if is_key_pressed(KeyCode::C) {
                settings.color = if settings.color == ColorMode::Speed { ColorMode::Mass } else { ColorMode::Speed };
                self.say(&format!("Color shows {}", settings.color.name()));
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
            if is_key_pressed(KeyCode::O) {
                settings.shell_prediction = !settings.shell_prediction;
            }
            if is_key_pressed(KeyCode::F) {
                settings.camera = match settings.camera {
                    Camera::Selection => Camera::Auto,
                    Camera::Auto => Camera::Selection,
                };
                self.watch = None;
                self.say(match settings.camera {
                    Camera::Selection => "Following selection",
                    Camera::Auto => "Auto camera",
                });
            }
            self.pick_target |= is_key_pressed(KeyCode::R);
            refocus = is_key_pressed(KeyCode::V);
            if is_key_pressed(KeyCode::B) {
                self.shop_open = !self.shop_open;
                shop_asked = self.shop_open;
            }
            if self.shop_open {
                for (key, upgrade) in SHOP_KEYS.into_iter().zip(Upgrade::SHIP.into_iter().chain(Upgrade::BASE)) {
                    if is_key_pressed(key) {
                        self.net.session.buy(Ware::Upgrade(upgrade));
                    }
                }
            }
            // The supplies the ship carries.
            if is_key_pressed(KeyCode::Q) {
                self.net.session.use_supply(Supply::Warp, 0, 0.0);
            }
            if is_key_pressed(KeyCode::E) {
                well_asked = true;
            }
            // The hotbar: 1 to 5 use the supplies carried (in the shop, they buy).
            if !self.shop_open {
                for (key, supply) in SHOP_KEYS.into_iter().zip(Supply::CARRIED) {
                    if !is_key_pressed(key) {
                        continue;
                    }
                    if supply == Supply::Well {
                        well_asked = true;
                    } else {
                        self.net.session.use_supply(supply, 0, 0.0);
                    }
                }
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
            if settings.mouse_aim {
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
            if well_asked {
                let (angle, speed) = self.shell_aim(&ship, (mwx, mwy));
                self.net.session.use_supply(Supply::Well, angle, speed as f32);
            }
        }
        // Held long enough, the self-destruct key gives the ship up, once per press.
        let held = keys && is_key_down(KeyCode::Backspace) && self.net.session.world.as_ref().is_some_and(|w| w.my_ship().is_some());
        self.destruct_done &= is_key_down(KeyCode::Backspace);
        self.destruct = if held && !self.destruct_done { self.destruct.or(Some(get_time())) } else { None };
        if self.destruct.is_some_and(|t| get_time() - t >= DESTRUCT_HOLD) {
            self.net.session.send_chat("/respawn");
            (self.destruct, self.destruct_done) = (None, true);
        }
        if settings.ship_color != self.color_sent && !is_mouse_button_down(MouseButton::Left) {
            self.color_sent = settings.ship_color;
            self.net.session.set_color(self.color_sent);
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
        if self.round != Some(world.round) {
            // A new world: nothing chosen or drawn in the old one means anything in it.
            self.round = Some(world.round);
            self.selected = None;
            self.picked = None;
            self.watch = None;
            self.frames.clear();
            self.last_frame_tick = 0;
            self.flashes.clear();
            self.predictor.clear();
            self.offset = (0.0, 0.0);
            self.zoomed_for_ship = false;
        }
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
        // A piece taken or gone, or a ship gone or no longer locked on to, is picked no more.
        let locked = world.me().is_some_and(|m| m.lock);
        self.picked = self.picked.filter(|k| match *k {
            RefKey::Piece(id) => world.pieces.contains_key(&id),
            RefKey::Ship(id) => locked && world.ship_at(id, tick_f).is_some(),
            RefKey::Body(_) => false,
        });
        // What predictions, the camera and the selection box go by: the pick, else the body.
        let reference: Option<RefKey> = self.picked.or(self.selected.map(RefKey::Body));
        let at_ref = |k: RefKey| match k {
            RefKey::Body(s) => Some(body(s)),
            RefKey::Piece(id) => world.piece_at(id, tick_f),
            RefKey::Ship(id) => world.ship_at(id, tick_f),
        };
        let own = world.ship_at(world.my_id, tick_f);
        // Watching: no ship of one's own, and the camera goes from one player to the next.
        let spectating = world.me().is_some_and(|m| m.spectating);
        // The targets of the round, the nearest to the ship first.
        let mut targets: Vec<Objective> = world.objectives.iter().filter(|o| alive(o.slot) && world.next_round_tick.is_none()).copied().collect();
        if let Some(ship) = own {
            let far = |o: &Objective| {
                let b = body(o.slot);
                (b.x - ship.x).powi(2) + (b.y - ship.y).powi(2)
            };
            targets.sort_by(|a, b| far(a).total_cmp(&far(b)));
        }
        // In a round of bases the standing enemy bases take the place of targets, nearest first.
        let bases = world.mode == GameMode::Bases;
        // The shop: at home only (or there waiting for a ship), in a round of bases.
        let playing = world.me().is_some_and(|m| !m.spectating) && world.next_round_tick.is_none();
        let waiting = world.me().is_some_and(|m| m.ship.is_none() && m.respawn_tick.is_some());
        let can_shop = bases && playing && world.base_of(world.my_id).is_some_and(|b| (waiting && b.hp > 0) || world.in_zone(world.my_id));
        if self.shop_open && !can_shop {
            self.shop_open = false;
            if shop_asked {
                let why = if bases { "The shop is open only at home" } else { "There is no shop in this game" };
                self.status = Some((why.to_string(), get_time() + 2.0));
            }
        }
        let my_team = world.team_of(world.my_id);
        // A team's color: with teams, the team's own; otherwise its player's.
        let team_color = |team: TeamId| {
            if world.in_teams() {
                style::team_color(team)
            } else {
                world.members(team).first().and_then(|id| world.players.get(id)).map_or(style::DIM, |p| style::rgb(p.color))
            }
        };
        let mut enemy_bases: Vec<u32> = world.bases.iter().filter(|b| b.hp > 0 && b.team != my_team && alive(b.slot)).map(|b| b.slot).collect();
        if let Some(ship) = own {
            let far = |slot: &u32| {
                let b = body(*slot);
                (b.x - ship.x).powi(2) + (b.y - ship.y).powi(2)
            };
            enemy_bases.sort_by(|a, b| far(a).total_cmp(&far(b)));
        }
        let picks: Vec<u32> = if bases {
            // Enemy bases and mines, nearest first.
            let mut picks: Vec<u32> = enemy_bases.iter().copied().chain(world.mines.iter().map(|s| s.slot).filter(|s| alive(*s))).collect();
            if let Some(ship) = own {
                let far = |slot: &u32| {
                    let b = body(*slot);
                    (b.x - ship.x).powi(2) + (b.y - ship.y).powi(2)
                };
                picks.sort_by(|a, b| far(a).total_cmp(&far(b)));
            }
            picks
        } else {
            targets.iter().map(|o| o.slot).collect()
        };
        if std::mem::take(&mut self.pick_target) {
            // The nearest one, then the next one each time.
            let after = self.selected.and_then(|s| picks.iter().position(|p| *p == s)).map_or(0, |i| i + 1);
            let text = match picks.get(after % picks.len().max(1)) {
                Some(&slot) => {
                    self.selected = Some(slot);
                    self.picked = None;
                    self.watch = None;
                    format!("Selected {}", world.body_name(slot))
                }
                None if bases => "No enemy base".to_string(),
                None => "No target".to_string(),
            };
            self.status = Some((text, get_time() + 2.0));
        }
        // The enemy base the siege panel is about: the selected one, else the nearest.
        let siege_of = self.selected.filter(|s| enemy_bases.contains(s)).or(enemy_bases.first().copied());
        // The one the panel is about: the one being held, else the selected, else the nearest.
        let holding = world.holds.get(&world.my_id).map(|h| h.0);
        let objective = holding.into_iter().chain(self.selected).find_map(|s| targets.iter().find(|o| o.slot == s)).or(targets.first()).copied();

        // --- camera ----------------------------------------------------------------------------
        if spectating && keys && is_key_pressed(KeyCode::Space) {
            let flying: Vec<PlayerId> = world.players.iter().filter(|(id, p)| **id != world.my_id && p.ship.is_some()).map(|(id, _)| *id).collect();
            let at = self.watch.and_then(|w| flying.iter().position(|id| *id == w));
            self.watch = flying.get(at.map_or(0, |a| a + 1) % flying.len().max(1)).copied();
        }
        let watched = self.watch.and_then(|id| world.ship_at(id, tick_f).map(|p| (id, p)));
        if watched.is_none() {
            self.watch = None;
        }
        let target = match (own, self.selected) {
            _ if watched.is_some() => Target::Player(watched.unwrap().0),
            _ if self.picked.is_some() => Target::Picked(self.picked.unwrap()),
            (_, Some(s)) => Target::Body(s),
            (Some(_), _) => Target::Ship,
            _ => Target::Free,
        };
        let target_pos = match target {
            Target::Ship => own.map(|p| (p.x, p.y)).unwrap(),
            Target::Player(_) => watched.map(|(_, p)| (p.x, p.y)).unwrap(),
            Target::Body(s) => {
                let b = body(s);
                (b.x, b.y)
            }
            Target::Picked(k) => at_ref(k).map_or(self.last_target_pos, |p| (p.x, p.y)),
            Target::Free => self.last_target_pos,
        };
        if !self.view_ready {
            // Start wide enough to see the neighborhood.
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
        // Stepping in by itself: only for our own ship, and only while there is one.
        let auto = settings.camera == Camera::Auto && own.is_some() && matches!(target, Target::Ship | Target::Body(_));
        if let (Target::Body(_), Some(ship), false, false) = (target, own, self.had_ship, auto) {
            frame_both(&mut self.view, &mut self.offset, target_pos, (ship.x, ship.y));
        }
        if let (true, Some(ship)) = (refocus, own) {
            self.watch = None;
            if matches!(target, Target::Body(_) | Target::Picked(_)) {
                // The ship and what is selected, together.
                fit_both(&mut self.view, &mut self.offset, target_pos, (ship.x, ship.y));
                self.view.mpp = self.view.mpp.max(FOCUS_MPP.0);
            } else {
                // The ship in the middle, at a scale that shows its neighborhood.
                self.offset = (0.0, 0.0);
                self.view.mpp = self.view.mpp.clamp(FOCUS_MPP.0, FOCUS_MPP.1);
            }
            self.auto.hands_off_until = 0.0;
        }
        self.had_ship = own.is_some();

        let pointer = !self.ui_has_pointer && !self.menu_open;
        let wheel = mouse_wheel().1 * WHEEL;
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
            self.view.mpp = (self.view.mpp * 1.1f64.powf(-step as f64)).clamp(MIN_MPP, 1.0e12);
            self.view.cx = target_pos.0 + self.offset.0;
            self.view.cy = target_pos.1 + self.offset.1;
            let after = self.view.to_world(mouse.0, mouse.1);
            self.offset.0 += before.0 - after.0;
            self.offset.1 += before.1 - after.1;
            if auto {
                // Like a drag, the wheel puts the view where the player wants it for now.
                (self.auto.hands_off_until, self.auto.moving) = (get_time() + AUTO_HANDS_OFF, false);
            }
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
                // The automatic camera lets a view chosen by hand stand for a while.
                (self.auto.hands_off_until, self.auto.moving) = (get_time() + AUTO_HANDS_OFF, false);
            }
        }
        self.last_mouse = mouse;
        match own.filter(|_| auto) {
            Some(ship) => {
                // Everything to keep in sight, measured from what the camera stays with.
                let mut seen = Bounds::default();
                seen.add(ship.x - target_pos.0, ship.y - target_pos.1, 0.0);
                let center = reference.and_then(|k| Some((at_ref(k)?, if let RefKey::Body(s) = k { row.props.radius[s as usize] } else { 0.0 })));
                if let Some((b, radius)) = center {
                    seen.add(b.x - target_pos.0, b.y - target_pos.1, radius);
                }
                let paths = &self.predictor.latest;
                let framed = (reference, self.round);
                let fresh = self.auto.framed != Some(framed);
                // The predicted path is looked at only now and then: it shifts with every touch
                // of the engine, and the view should not.
                let now = get_time();
                let thrusting = controls.thrust > 0;
                // Paths are kept as seen from the selected body (or in the world, without one).
                let from = center.map_or((0.0, 0.0), |(b, _)| (b.x, b.y));
                let shift = (from.0 - target_pos.0, from.1 - target_pos.1);
                if paths.reference != reference {
                    self.auto.path = None;
                } else if thrusting && !fresh {
                    // Whatever the burn makes of the path is looked at shortly after it ends.
                    self.auto.path_at = now - AUTO_PATH_EVERY + AUTO_AFTER_BURN;
                } else if fresh || self.auto.path.is_none() || now - self.auto.path_at >= AUTO_PATH_EVERY {
                    // Only as far as the closest approach to the selected body: what comes after
                    // is another matter (the whole line when there is no such point).
                    let upto = paths.closest.map_or(paths.coast.len(), |(i, _)| i + 1);
                    let mut path = Bounds::default();
                    for q in paths.coast.iter().take(upto).chain(&paths.shell) {
                        path.add(q.0, q.1, 0.0);
                    }
                    // A path that goes out and comes back is an orbit: all of it fits in a circle
                    // around the body as wide as its farthest point, and that view can stay.
                    if let Some((_, far)) = paths.farthest.filter(|_| paths.returns) {
                        path.add(0.0, 0.0, far);
                    }
                    (self.auto.path, self.auto.path_at) = (Some(path), now);
                }
                if let Some(path) = self.auto.path.filter(|p| p.min.0 <= p.max.0) {
                    seen.add(path.min.0 + shift.0, path.min.1 + shift.1, 0.0);
                    seen.add(path.max.0 + shift.0, path.max.1 + shift.1, 0.0);
                }
                self.auto.framed = Some(framed);
                self.auto.steer(&seen, &mut self.view.mpp, &mut self.offset, fresh, thrusting, dt as f64);
            }
            None => self.auto.framed = None,
        }
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
                // A piece, or (locked on) another ship, nearer the cursor than any body is picked.
                let mut pick: Option<(RefKey, f32)> = None;
                let mut consider = |k: RefKey, p: Particle| {
                    let s = view.to_screen(p.x, p.y);
                    let d = ((s.0 - mouse.0).powi(2) + (s.1 - mouse.1).powi(2)).sqrt();
                    if d <= PICK_RADIUS_PX * ui && pick.is_none_or(|(_, bd)| d < bd) {
                        pick = Some((k, d));
                    }
                };
                for id in world.pieces.keys() {
                    if let Some(p) = world.piece_at(*id, tick_f) {
                        consider(RefKey::Piece(*id), p);
                    }
                }
                for id in world.players.keys().filter(|id| locked && **id != world.my_id) {
                    if let Some(p) = world.ship_at(*id, tick_f) {
                        consider(RefKey::Ship(*id), p);
                    }
                }
                match (pick, best) {
                    (Some((k, d)), b) if b.is_none_or(|b| d < b.1) => (self.picked, self.selected) = (Some(k), None),
                    _ => (self.picked, self.selected) = (None, best.map(|b| b.0)),
                }
                self.watch = None;
            }
        } else if !dragging {
            self.drag_from = None;
        }

        let mut labels = style::Labels::default();

        // --- bodies as light ---------------------------------------------------------------------
        // Bodies too small on screen to have a shape are drawn as glowing points, the way the
        // large-scale worlds draw theirs; the rest (and negative masses, which have their own
        // colors) get their discs below.
        self.density.set_gpu(settings.gpu);
        let mut scene = gsim_swarm::Bodies::default();
        for j in (0..row.x.len() as u32).filter(|j| alive(*j) && row.props.mass[*j as usize] > 0.0) {
            let b = body(j);
            scene.push(b.x, b.y, b.vx, b.vy, row.props.mass[j as usize], row.props.radius[j as usize], 0);
            *scene.id.last_mut().unwrap() = j;
        }
        // Merges flash where they happened.
        for t in self.flash_tick.max(world.head.saturating_sub(60))..world.head {
            let Some(r) = world.eph.get(t) else { continue };
            for e in &r.merges {
                if let Some(s) = e.survivor.map(|s| s as usize) {
                    let mass: f64 = e.absorbed.iter().map(|a| r.props.mass[*a as usize].abs()).sum();
                    self.flashes.push((r.x[s], r.y[s], r.vx[s], r.vy[s], mass as f32, t));
                }
            }
        }
        self.flash_tick = world.head;
        self.flashes.retain(|f| tick_f - (f.5 as f64) < FLASH_TICKS);
        let flashes: Vec<Flash> = self
            .flashes
            .iter()
            .filter(|f| tick_f >= f.5 as f64)
            .map(|f| {
                let age = tick_f - f.5 as f64;
                let moved = age * world.rules.dt;
                Flash { x: f.0 + f.2 * moved, y: f.1 + f.3 * moved, life: (1.0 - age / FLASH_TICKS) as f32, mass: f.4 }
            })
            .collect();
        let mut shaped = vec![false; row.x.len()];
        for big in self.density.project(&scene, &view, 0.0, if settings.color == ColorMode::Speed { ColorMode::Speed } else { ColorMode::Mass }) {
            shaped[big.id as usize] = true;
        }
        self.density.present(&flashes);
        if let Some(why) = self.density.take_notice() {
            self.status = Some((format!("Unable to draw on the graphics card: {why}"), get_time() + 6.0));
            settings.gpu = false;
            settings.save();
        }

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
        // Reference frame for trails and predictions: the pick or the selected body, if any.
        // (Trails go only by bodies, whose past the ephemeris keeps.)
        let ref_now = reference.and_then(at_ref);
        if settings.trails {
            let by = self.selected.filter(|_| settings.trails_relative && self.picked.is_none());
            draw_trails(&self.frames, &view, by, by.map(&body), &row, world.my_id, ui);
        }

        // --- bodies ----------------------------------------------------------------------------
        let row_view = row.view();
        let frame = SystemFrame::of(&row_view);
        let escape_radius = world.rules.escape_radius;
        // Planets are lit by the heaviest star there is.
        let star = (0..row.x.len() as u32)
            .filter(|j| alive(*j) && style::is_star(row.props.mass[*j as usize]))
            .max_by(|a, b| row.props.mass[*a as usize].total_cmp(&row.props.mass[*b as usize]))
            .map(|j| view.to_screen(body(j).x, body(j).y));
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
            let base = world.base_on(j).is_some();
            if (mass < 0.0 || shaped[j as usize]) && !base {
                style::body(s, r_px, mass, j, fade, ui, style::light_from(s, star));
            }
            if let Some(name) = world.names.get(&j).filter(|_| self.selected != Some(j) && !base) {
                let size = LABEL * ui;
                let below = s.1 + r_px.max(1.1 * ui).min(4000.0) + size;
                labels.push(name.as_str(), s.0, below, size, style::alpha(style::DIM, fade), Rank::BodyName);
            }
        }

        // --- bases ----------------------------------------------------------------------------
        for b in &world.bases {
            let j = b.slot as usize;
            if !alive(b.slot) {
                continue;
            }
            let at = body(b.slot);
            let s = view.to_screen(at.x, at.y);
            let radius = row.props.radius[j];
            let r_px = (radius / view.mpp) as f32;
            let members = world.members(b.team);
            let color = team_color(b.team);
            let label = world.team_label(b.team);
            let name = label.as_str();
            let mine = b.team == my_team && my_team != 0;
            let health = b.hp as f32 / BASE_HP as f32;
            // Around one's own base: the home zone, where shells cost nothing (brighter while
            // inside it), drawn even when the base itself is off screen. Around the others:
            // where a siege orbit has to stay.
            let zone_radii = members.first().and_then(|id| world.players.get(id)).map_or(gsim_proto::HOME_RADII, |p| shop::zone_radii(&p.upgrades));
            let home = (zone_radii * radius / view.mpp) as f32;
            let outer = (world.rules.orbit_max_apo_radii * radius / view.mpp) as f32;
            if mine && home > 14.0 * ui {
                let lit = world.in_zone(world.my_id);
                if home < 1.0e6 {
                    style::zone(s, home, color, lit, ui);
                } else {
                    // Zoomed far in: worked out in the world, where the numbers are exact.
                    let zone = zone_radii * radius;
                    let (dx, dy) = (view.cx - at.x, view.cy - at.y);
                    let d = (dx * dx + dy * dy).sqrt().max(1.0);
                    let half = 0.5 * ((screen_width() as f64).powi(2) + (screen_height() as f64).powi(2)).sqrt() * view.mpp;
                    if d + half <= zone {
                        draw_rectangle(0.0, 0.0, screen_width(), screen_height(), style::alpha(color, if lit { 0.07 } else { 0.035 }));
                    } else if d - half < zone {
                        let edge = view.to_screen(at.x + dx / d * zone, at.y + dy / d * zone);
                        style::zone_edge(edge, (-(dx / d) as f32, -(dy / d) as f32), color, lit, ui);
                    }
                }
            }
            if b.hp > 0 && !mine && outer > 14.0 * ui && outer < 1.0e5 && view.on_screen(s, outer) {
                let inner = (world.rules.orbit_min_peri_radii * radius / view.mpp) as f32;
                let dashes = 72;
                for k in 0..dashes {
                    let a = k as f32 / dashes as f32 * std::f32::consts::TAU;
                    style::arc(s.0, s.1, outer, a, a + 0.045, ui, style::alpha(color, 0.3));
                }
                style::ring(s.0, s.1, inner, ui, style::alpha(color, 0.2));
            }
            if !view.on_screen(s, r_px.max(30.0 * ui)) {
                // Off screen: an arrow to every standing base, one's own included.
                if b.hp > 0 {
                    // A ring on the arrow tells it from a ship's, and the label sits further in so
                    // that it does not cover the owner's ship label when both point the same way.
                    let label = edge_arrow(s, color, ui);
                    let (cx, cy) = (screen_width() * 0.5, screen_height() * 0.5);
                    let (dx, dy) = (cx - label.0, cy - label.1);
                    let k = 30.0 * ui / (dx * dx + dy * dy).sqrt().max(1.0);
                    let (ex, ey) = (label.0 - dx * 34.0 / 30.0 * k, label.1 - dy * 34.0 / 30.0 * k);
                    style::ring(ex, ey, 13.0 * ui, 1.5 * ui, style::alpha(color, 0.8));
                    let at_edge = (label.0 + dx * k, label.1 + dy * k);
                    let mut lines = vec![if mine { "HOME".to_string() } else { format!("{} BASE", name.to_uppercase()) }];
                    if let Some(ship) = own {
                        lines.push(fmt::distance(((at.x - ship.x).powi(2) + (at.y - ship.y).powi(2)).sqrt()));
                    }
                    labels.block(lines, at_edge.0, at_edge.1 - 8.0 * ui, LABEL * ui, color, Rank::Target);
                }
                continue;
            }
            let besieged = world.sieges.iter().any(|x| x.1 == b.team);
            style::base(s, r_px, color, health, besieged, b.slot, ui);
            let top = r_px.max(6.0 * ui).min(4000.0) + 16.0 * ui;
            if b.hp > 0 {
                labels.push(if mine { "HOME".to_string() } else { name.to_uppercase() }, s.0, s.1 - top - 4.0 * ui, LABEL * ui, color, Rank::Target);
            } else {
                labels.push(format!("{} · DOWN", name.to_uppercase()), s.0, s.1 - top, LABEL * ui, style::DIM, Rank::Target);
            }
        }

        // --- mines ------------------------------------------------------------------------------
        // Ticks on the orbit around a mine for each piece it gives.
        let mine_every = |kind: PieceKind| (mine_seconds(kind) * world.rules.tick_hz as f64) as u32;
        for sp in world.mines.iter().filter(|sp| alive(sp.slot) && world.next_round_tick.is_none()) {
            let j = sp.slot as usize;
            let at = body(sp.slot);
            let s = view.to_screen(at.x, at.y);
            let radius = row.props.radius[j];
            let color = style::piece_color(sp.kind);
            let name = if sp.kind == PieceKind::Scrap { "SCRAP MINE" } else { "CRYSTAL MINE" }.to_string();
            if !view.on_screen(s, 0.0) {
                let at_edge = edge_arrow(s, color, ui);
                let mut lines = vec![name];
                if let Some(ship) = own {
                    lines.push(fmt::distance(((at.x - ship.x).powi(2) + (at.y - ship.y).powi(2)).sqrt()));
                }
                labels.block(lines, at_edge.0, at_edge.1 - 8.0 * ui, LABEL * ui, color, Rank::Target);
                continue;
            }
            // The band a qualifying orbit must stay inside.
            let outer = (world.rules.orbit_max_apo_radii * radius / view.mpp) as f32;
            if outer > 14.0 * ui {
                let inner = (world.rules.orbit_min_peri_radii * radius / view.mpp) as f32;
                style::ring(s.0, s.1, outer.min(1.0e5), ui, style::alpha(color, 0.3));
                style::ring(s.0, s.1, inner.min(1.0e5), ui, style::alpha(color, 0.3));
            }
            let every = mine_every(sp.kind).max(1);
            let held = world.holds.get(&world.my_id).filter(|h| h.0 == sp.slot).map_or(0.0, |h| (h.1 % every) as f32 / every as f32);
            let r_px = (radius / view.mpp) as f32;
            style::mine(s, r_px, sp.kind, sp.left, held, ui);
            labels.push(name, s.0, s.1 - r_px.min(4000.0) - 26.0 * ui, LABEL * ui, color, Rank::Target);
        }

        // --- shells and effects ----------------------------------------------------------------
        for (id, shell) in &world.shells {
            let Some(p) = world.shell_at(*id, tick_f) else { continue };
            let blast_px = (shell.blast / view.mpp) as f32;
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
        // Scrap and crystals, each turning at its own pace; within reach of the ship, how close
        // it has to pass to take them.
        let reach_px = (PICKUP_RADIUS / view.mpp) as f32;
        let spin = get_time() as f32;
        for (id, piece) in &world.pieces {
            let Some(p) = world.piece_at(*id, tick_f) else { continue };
            let s = view.to_screen(p.x, p.y);
            if !view.on_screen(s, reach_px.max(10.0 * ui)) {
                continue;
            }
            if reach_px > 14.0 * ui && reach_px < 1.0e5 {
                style::ring(s.0, s.1, reach_px, ui, style::alpha(style::piece_color(piece.kind), 0.12));
            }
            // Fading away, far out with nobody near.
            let fade = piece.fading.map_or(1.0, |since| (1.0 - (tick_f - since as f64) / (shop::FADE_SECONDS * world.rules.tick_hz as f64)).clamp(0.0, 1.0) as f32);
            style::piece(s, piece.kind, spin * (0.7 + (id % 5) as f32 * 0.3) + *id as f32, fade, ui);
        }
        // Gravity wells.
        let well_ticks = world.rules.well_ticks as f64;
        // Drawn out to where it pulls as hard as the engine pushes.
        let thrust = world.rules.burn_per_tick_mmps as f64 * 1.0e-3 / world.rules.dt;
        let well_reach = (world.rules.g * world.rules.well_mass / thrust).sqrt();
        let well_arm = world.rules.well_arm_ticks as f64;
        for w in &world.wells {
            let Some(p) = w.at(tick_f as Tick) else { continue };
            let s = view.to_screen(p.x, p.y);
            let age = tick_f - w.fired as f64;
            if age < well_arm {
                // On its way: a dark seed, gathering.
                style::well_seed(s, (age / well_arm) as f32, ui);
            } else if age < well_arm + well_ticks {
                style::well(s, (well_reach / view.mpp) as f32, (1.0 - (age - well_arm) / well_ticks) as f32, ui);
            }
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
                EffectKind::ShellBlast(blast) => style::disc(s.0, s.1, ((blast / view.mpp) as f32).max(6.0), Color::new(1.0, 0.6, 0.3, 0.5 * fade)),
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
                EffectKind::Warped => style::warp(s, style::ACCENT, (age as f32 / 0.6).min(1.0), ui),
                EffectKind::Emp => style::emp(s, (shop::EMP_RADIUS / view.mpp) as f32, (age as f32 / 0.8).min(1.0), ui),
                EffectKind::Mined => {
                    // Rings bursting outwards in the crystals' color.
                    let c = style::piece_color(PieceKind::Crystal);
                    for delay in [0.0f32, 0.15, 0.3] {
                        let t = (age as f32 - delay) / 1.1;
                        if (0.0..1.0).contains(&t) {
                            style::ring(s.0, s.1, (12.0 + 90.0 * t) * ui, (3.5 - 2.5 * t) * ui, style::alpha(c, (1.0 - t) * (1.0 - t)));
                        }
                    }
                    style::disc(s.0, s.1, 22.0 * ui, style::alpha(c, 0.4 * (1.0 - (age as f32 / 0.4).min(1.0))));
                }
                EffectKind::ShieldBroken => style::shield_break(s, (age as f32 / 0.8).min(1.0), ui),
                EffectKind::Pickup(kind) => style::pickup(s, kind, (age as f32 / 0.5).min(1.0), ui),
                EffectKind::BaseDestroyed => {
                    // A flash, then three shock waves.
                    style::disc(s.0, s.1, 60.0 * ui, style::alpha(WHITE, 0.5 * (1.0 - (age as f32 / 0.3).min(1.0))));
                    for (k, c) in [style::EMBER, style::GOLD, WHITE].into_iter().enumerate() {
                        let t = (age as f32 - 0.12 * k as f32) / 1.3;
                        if (0.0..1.0).contains(&t) {
                            style::ring(s.0, s.1, (20.0 + 220.0 * t) * ui, (5.0 - 4.0 * t) * ui, style::alpha(c, (1.0 - t) * (1.0 - t)));
                        }
                    }
                }
                EffectKind::ShipDestroyed => {
                    let r = (10.0 + 60.0 * age as f32) * ui;
                    style::ring(s.0, s.1, r, 3.0 * ui, Color::new(1.0, 0.9, 0.5, fade));
                }
            }
        }

        // --- predictions -----------------------------------------------------------------------
        // What a frame has to spare, for a client that predicts between frames.
        let spare = self.period - self.busy - SLACK;
        let asked = fmt::now();
        if self.predictor.poll(spare) {
            // The path of one's own ship; while watching, of the player followed.
            let subject = world.players.get(&if spectating { self.watch.unwrap_or(world.my_id) } else { world.my_id });
            let ship = subject.and_then(|p| p.ship.as_ref()).filter(|t| t.crashed.is_none());
            let wanted = settings.prediction || settings.shell_prediction;
            match (ship, subject) {
                (Some(track), Some(me)) if wanted => {
                    let shell = own.filter(|_| settings.shell_prediction).and_then(|ship| {
                        let (angle, speed) = self.shell_aim(&ship, (mwx, mwy));
                        world.shell_muzzle(angle, speed)
                    });
                    self.predictor.submit(Job {
                        reader: world.eph.reader(),
                        rules: world.rules.clone(),
                        ship: *track.last(),
                        wells: world.well_paths(),
                        start: track.end(),
                        timeline: me.timeline.clone(),
                        ticks: self.lookahead_ticks,
                        show_ship: settings.prediction,
                        held: !spectating && controls.thrust > 0,
                        burn_mmps: (settings.burn_preview as f64 * 1.0e6) as i64,
                        shell,
                        reference: reference.and_then(|k| {
                            // Where it is as the path starts.
                            let at = track.end();
                            Some(match k {
                                RefKey::Body(s) => Reference::Body(s),
                                RefKey::Piece(id) => {
                                    let t = &world.pieces.get(&id)?.track;
                                    Reference::Piece(id, *t.at(at).unwrap_or(t.last()))
                                }
                                RefKey::Ship(id) => {
                                    let t = world.players.get(&id)?.ship.as_ref()?;
                                    Reference::Ship(id, *t.at(at).unwrap_or(t.last()))
                                }
                            })
                        }),
                    });
                }
                _ => self.predictor.clear(),
            }
        }
        let predicting = fmt::now() - asked;
        let paths = &self.predictor.latest;
        // A result computed for another reference frame would be drawn in the wrong place.
        let stale = paths.reference != reference;
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
        if let (false, Some((i, d))) = (stale, paths.farthest) {
            // Where the path turns back towards the selected body.
            let q = paths.coast[i];
            let s = view.to_screen(anchor.0 + q.0, anchor.1 + q.1);
            let c = Color::from_rgba(130, 200, 255, 255);
            style::ring(s.0, s.1, 5.0 * ui, 1.5 * ui, c);
            let eta = i as f64 / world.rules.tick_hz as f64;
            labels.block(vec![format!("farthest {}", fmt::distance(d)), format!("in {eta:.1} s")], s.0, s.1 + 18.0 * ui, LABEL * ui, c, Rank::Approach);
        }

        // --- ship wakes ------------------------------------------------------------------------
        // A short fading tail behind every ship, in the same frame as the prediction line
        // (relative to the selected body, if any). The full trails replace it when switched on.
        let t0 = tick_f.floor() as Tick;
        if !settings.trails {
            let span = (2.5 * world.rules.tick_hz as f64) as Tick;
            for p in world.players.values() {
                let Some(track) = p.ship.as_ref() else { continue };
                let color = style::rgb(p.color);
                let newest = t0.min(track.end());
                let oldest = newest.saturating_sub(span).max(track.base);
                let mut prev: Option<(f32, f32)> = None;
                for t in (oldest..=newest).step_by(2) {
                    let Some(state) = track.at(t) else { continue };
                    let shift = match (reference, ref_now) {
                        (Some(RefKey::Body(r)), Some(now)) => world
                            .eph
                            .get(t)
                            .filter(|row| row.props.alive[r as usize])
                            .map(|row| (now.x - row.x[r as usize], now.y - row.y[r as usize])),
                        (Some(RefKey::Piece(id)), Some(now)) => world.pieces.get(&id).and_then(|q| q.track.at(t)).map(|q| (now.x - q.x, now.y - q.y)),
                        (Some(RefKey::Ship(id)), Some(now)) => world.players.get(&id).and_then(|q| q.ship.as_ref()?.at(t)).map(|q| (now.x - q.p.x, now.y - q.p.y)),
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
                // Other players out of view: an arrow on the edge, as for targets.
                if let Some(me) = own.filter(|_| *id != world.my_id) {
                    let at = edge_arrow(s, style::rgb(p.color), ui);
                    let d = ((ship.x - me.x).powi(2) + (ship.y - me.y).powi(2)).sqrt();
                    labels.block(vec![p.name.clone(), fmt::distance(d)], at.0, at.1 - 8.0 * ui, LABEL * ui, style::rgb(p.color), Rank::Player);
                }
                continue;
            }
            let mine = *id == world.my_id;
            let input = p.timeline.at(t0);
            // Our own heading is shown instantly; the simulation follows a few ticks later.
            let facing = if mine { self.heading } else { math::angle_to_radians(input.angle) };
            let fuel = p.ship.as_ref().map_or(0, |t| t.last().fuel);
            let color = style::rgb(p.color);
            draw_ship(s, facing as f32, color, input.thrust > 0 && fuel > 0, ui);
            if (p.stunned.0..p.stunned.1).contains(&t0) {
                // Engines dead (an EMP): sparks about it.
                style::sparks(s, ui);
            }
            if (t0 as f64) < p.magnet_until as f64 {
                // A scrap magnet: how far it reaches.
                let reach = (shop::MAGNET_REACH * PICKUP_RADIUS / view.mpp) as f32;
                style::ring(s.0, s.1, reach.max(16.0 * ui), ui, style::alpha(style::piece_color(PieceKind::Scrap), 0.35));
            }
            if world.in_teams() && p.team != 0 {
                // Which side it is on: a ring in the team's color.
                style::ring(s.0, s.1, 12.0 * ui, 1.2 * ui, style::alpha(style::team_color(p.team), 0.75));
            }
            if p.shield {
                style::ship_shield(s, *id, ui);
            }
            if !mine {
                labels.push(p.name.as_str(), s.0, s.1 + 20.0 * ui, LABEL * ui, color, Rank::Player);
            } else {
                // How fast, and how fast against what is selected.
                let mut text = fmt::speed(ship.vx.hypot(ship.vy));
                if let Some(r) = ref_now {
                    text = format!("{text} · rel {}", fmt::speed((ship.vx - r.vx).hypot(ship.vy - r.vy)));
                }
                labels.push(text, s.0, s.1 + 20.0 * ui, LABEL * ui, style::DIM, Rank::Player);
            }
        }

        // --- objective -------------------------------------------------------------------------
        let gold = style::GOLD;
        let clock = |s: f64| format!("{}:{:02}", s as u32 / 60, s as u32 % 60);
        let target_secs = |o: &Objective| o.left as f64 / world.rules.tick_hz as f64;
        let mut orbit = None;
        for o in &targets {
            let slot = o.slot;
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
                let label = if world.rules.target_ticks > 0 { format!("TARGET {}", clock(target_secs(o))) } else { "TARGET".to_string() };
                labels.push(label, s.0, s.1 - ring - 5.0 * ui, LABEL * ui, gold, Rank::Target);
            } else {
                // Off screen: an arrow on the edge pointing at it.
                let at = edge_arrow(s, gold, ui);
                if let Some(ship) = own {
                    let d = ((b.x - ship.x).powi(2) + (b.y - ship.y).powi(2)).sqrt();
                    labels.push(fmt::distance(d), at.0, at.1, LABEL * ui, gold, Rank::Target);
                }
            }
            if let Some(ship) = own.filter(|_| objective.is_some_and(|t| t.slot == slot)) {
                orbit = Some((orbit_status(&ship, &b, row.props.mass[slot as usize], radius, &world.rules), radius));
            }
        }

        // --- selection info --------------------------------------------------------------------
        if let (Some(k), Some(p)) = (self.picked, ref_now) {
            let s = view.to_screen(p.x, p.y);
            let r_px = 10.0 * ui;
            draw_rectangle_lines(s.0 - r_px, s.1 - r_px, 2.0 * r_px, 2.0 * r_px, 1.5 * ui, WHITE);
            let mut lines = vec![match k {
                RefKey::Piece(id) => match world.pieces.get(&id).map(|q| q.kind) {
                    Some(PieceKind::Crystal) => "crystal".to_string(),
                    Some(PieceKind::Crate) => "supply crate".to_string(),
                    _ => "scrap".to_string(),
                },
                RefKey::Ship(id) => world.player_name(id).to_string(),
                RefKey::Body(s) => world.body_name(s),
            }];
            lines.push(format!("v = {}", fmt::speed(p.vx.hypot(p.vy))));
            if let Some(ship) = own {
                lines.push(format!("d = {}", fmt::distance((p.x - ship.x).hypot(p.y - ship.y))));
                lines.push(format!("rel v = {}", fmt::speed((p.vx - ship.vx).hypot(p.vy - ship.vy))));
            }
            if !settings.body_details {
                lines.truncate(1);
            }
            labels.block(lines, s.0, s.1 + r_px + 12.0 * ui, LABEL * ui, WHITE, Rank::Selection);
        }
        if let (Some(slot), Some(b), None) = (self.selected, ref_now, self.picked) {
            let s = view.to_screen(b.x, b.y);
            let r_px = ((row.props.radius[slot as usize] / view.mpp) as f32).min(4000.0) + 6.0 * ui;
            draw_rectangle_lines(s.0 - r_px, s.1 - r_px, 2.0 * r_px, 2.0 * r_px, 1.5 * ui, WHITE);
            // The details only with the setting on.
            if !settings.body_details {
                // A named body keeps its name label.
                if let Some(name) = world.names.get(&slot) {
                    labels.push(name.as_str(), s.0, s.1 + r_px + 12.0 * ui, LABEL * ui, WHITE, Rank::Selection);
                }
            } else {
            let mut lines = vec![world.body_name(slot)];
            lines.push(format!("m = {}", fmt::mass(row.props.mass[slot as usize])));
            lines.push(format!("r = {}", fmt::distance(row.props.radius[slot as usize])));
            lines.push(format!("v = {}", fmt::speed((b.vx * b.vx + b.vy * b.vy).sqrt())));
            if let Some(ship) = own {
                let d = ((b.x - ship.x).powi(2) + (b.y - ship.y).powi(2)).sqrt();
                let v = ((b.vx - ship.vx).powi(2) + (b.vy - ship.vy).powi(2)).sqrt();
                lines.push(format!("d = {}", fmt::distance(d)));
                lines.push(format!("rel v = {}", fmt::speed(v)));
            }
            labels.block(lines, s.0, s.1 + r_px + 12.0 * ui, LABEL * ui, WHITE, Rank::Selection);
            }
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
            labels.push(m.name.as_str(), s.0, s.1 - 10.0 * ui, LABEL * ui, style::alpha(style::ACCENT, fade), Rank::Player);
        }

        labels.draw();

        // --- HUD -------------------------------------------------------------------------------
        let me = world.me();
        let fuel = me.and_then(|m| m.ship.as_ref()).map(|t| t.last().fuel);
        let respawn_in = me.and_then(|m| m.respawn_tick).map(|t| (t as f64 - tick_f).max(0.0) / world.rules.tick_hz as f64);
        let reloading = fire_ready as f64 > tick_f;
        let shells_max = me.map_or(world.rules.shell_max, |m| m.magazine.max);
        let (shells, next_shell) = me.map_or((0, 0.0), |m| (m.magazine.at(tick_f as Tick, &world.rules), m.magazine.next(tick_f as Tick, &world.rules)));
        // In a round of bases the first column is the base's health, and the board is in that order.
        // With teams, a team's players together, the base's health in the team's color.
        let base_hp = |id: &PlayerId| world.base_of(*id).map_or(-1, |b| b.hp as i64);
        let mut scores: Vec<ScoreRow> = world
            .players
            .iter()
            .filter(|(_, p)| !p.spectating)
            .map(|(id, p)| ScoreRow {
                name: p.name.clone(),
                score: if bases { base_hp(id) } else { p.score(&world.rules) },
                counts: [p.captures, p.kills, p.deaths],
                mine: *id == world.my_id,
                color: style::rgb(p.color),
                team: p.team,
            })
            .collect();
        let teams_on = world.in_teams();
        let watchers: Vec<(String, Color)> = world.players.values().filter(|p| p.spectating).map(|p| (p.name.clone(), style::rgb(p.color))).collect();
        scores.sort_by(|a, b| b.score.cmp(&a.score).then(a.team.cmp(&b.team)).then(a.counts[2].cmp(&b.counts[2])));
        let secs = |t: Tick| (t as f64 - tick_f).max(0.0) / world.rules.tick_hz as f64;
        let round_left = world.round_end_tick.map(secs);
        // Bases: how long until sudden death (0 once it is on).
        let sudden_left = world.sudden_death.filter(|_| bases).map(secs);
        let intermission = world.next_round_tick.map(secs);
        let hold_goal = world.rules.hold_ticks as f32;
        let on_target = |h: &(u32, u32)| objective.is_some_and(|t| t.slot == h.0);
        let my_hold = world.holds.get(&world.my_id).filter(|h| on_target(h)).map_or(0, |h| h.1) as f32 / hold_goal;
        let respawn_secs = (world.rules.respawn_ticks as f64 / world.rules.tick_hz as f64).max(1e-3);
        let destruct = self.destruct.map(|t| ((get_time() - t) / DESTRUCT_HOLD) as f32);
        let rivals: Vec<(String, f32, Color)> = world
            .holds
            .iter()
            .filter(|(id, h)| **id != world.my_id && on_target(h))
            .map(|(id, h)| (world.player_name(*id).to_string(), h.1 as f32 / hold_goal, style::rgb(world.players[id].color)))
            .collect();
        let target_name = objective.map(|t| world.body_name(t.slot));
        // For each target: the time left, that as a share of the whole, and whether somebody
        // on the orbit is keeping it from running out.
        let target_span = world.rules.target_ticks as f32;
        let timer = |o: &Objective| (clock(target_secs(o)), o.left as f32 / target_span.max(1.0), world.holds.values().any(|h| h.0 == o.slot));
        let target_time = objective.as_ref().filter(|_| target_span > 0.0).map(timer);
        let others: Vec<(String, (String, f32, bool))> =
            targets.iter().filter(|o| objective.is_some_and(|t| t.slot != o.slot)).map(|o| (world.body_name(o.slot), timer(o))).collect();
        let limits = (world.rules.orbit_max_ecc, world.rules.orbit_min_peri_radii, world.rules.orbit_max_apo_radii);
        let capture_points = world.rules.capture_points;
        // Bases: one's own, who is besieging it, and the enemy base one is going for.
        let health = |b: &gsim_proto::Base| b.hp as f32 / BASE_HP as f32;
        let home = world.base_of(world.my_id).map(health);
        let besiegers: Vec<(String, Color)> = world
            .sieges
            .iter()
            .filter(|x| x.1 == my_team && my_team != 0)
            .map(|x| (world.player_name(x.0).to_string(), world.players.get(&x.0).map_or(style::DIM, |p| style::rgb(p.color))))
            .collect();
        let siege = siege_of.and_then(|slot| world.base_on(slot)).map(|b| {
            let name = world.team_label(b.team).to_uppercase();
            let color = team_color(b.team);
            let j = b.slot as usize;
            let state = own.map(|ship| orbit_status(&ship, &body(b.slot), row.props.pull[j], row.props.radius[j], &world.rules));
            let on = world.sieges.iter().any(|x| x.0 == world.my_id && x.1 == b.team);
            (name, color, health(b), state.map(|o| (o, row.props.radius[j])), on)
        });
        let out = spectating && bases && world.base_of(world.my_id).is_some_and(|b| b.hp == 0);
        // The mine one is holding an orbit around, and how far along to the next crystal.
        let mining = world.holds.get(&world.my_id).and_then(|h| {
            let kind = world.mines.iter().find(|m| m.slot == h.0)?.kind;
            let every = mine_every(kind).max(1);
            Some((kind, (h.1 % every) as f32 / every as f32))
        });
        // Crystals turning up soon.
        let crystals_in = world.crystals_due.map(|t| (t as f64 - tick_f) / world.rules.tick_hz as f64).filter(|s| (0.0..10.0).contains(s) && bases && world.next_round_tick.is_none());
        let winner = world.winner.map(|w| world.team_label(w));
        // Under the clock: what is going on around us right now, as gauges (what, how far, color),
        // then what just happened to us (fading out after a few seconds).
        let mut happening: Vec<(String, Option<String>, f32, Color)> = Vec::new();
        if let (false, Some(h)) = (besiegers.is_empty(), home) {
            let who: Vec<String> = besiegers.iter().map(|b| b.0.to_uppercase()).collect();
            happening.push((format!("UNDER SIEGE · {}", who.join(", ")), None, h, style::EMBER));
        }
        if let Some((name, color, h, _, true)) = &siege {
            happening.push((format!("SIEGE · {name}"), None, *h, *color));
        }
        if let Some((kind, frac)) = mining {
            happening.push(("MINING".to_string(), None, frac, style::piece_color(kind)));
        }
        if let Some(left) = crystals_in {
            happening.push(("CRYSTALS AT THE CENTER".to_string(), Some(format!("in {left:.0} s")), 1.0 - left as f32 / 10.0, style::piece_color(PieceKind::Crystal)));
        }
        if let (Some(name), true) = (&target_name, my_hold > 0.0) {
            happening.push((format!("CAPTURING {}", name.to_uppercase()), None, my_hold, style::GOLD));
        }
        let hz = world.rules.tick_hz as f64;
        let alerts: Vec<(String, Color, f32)> = world
            .alerts
            .iter()
            .map(|a| (a, (tick_f - a.tick as f64) / hz))
            .filter(|(_, age)| (0.0..ALERT_SECONDS).contains(age))
            .map(|(a, age)| (a.text.clone(), if a.good { style::GOOD } else { style::EMBER }, (ALERT_SECONDS - age).min(1.0) as f32))
            .collect();
        let free_shells = world.at_home(world.my_id);
        // What the chat box needs: names to complete and to highlight.
        let player_names: Vec<String> = world.players.values().map(|p| p.name.clone()).collect();
        let mut mentions: Vec<(String, Mention)> = world.players.iter().map(|(id, p)| (p.name.clone(), Mention::Player(*id))).collect();
        mentions.extend(world.names.iter().map(|(slot, name)| (name.clone(), Mention::Body(*slot))));
        // Unnamed bodies are worth mentioning when they are targets or selected.
        for slot in targets.iter().map(|o| o.slot).chain(self.selected) {
            if !world.names.contains_key(&slot) {
                mentions.push((world.body_name(slot), Mention::Body(slot)));
            }
        }
        let body_names: Vec<String> = mentions.iter().filter(|m| matches!(m.1, Mention::Body(_))).map(|m| m.0.clone()).collect();
        let presets: Vec<(&str, Vec<&str>)> = PRESETS.iter().map(|p| (p.name, p.params.iter().map(|s| s.key).collect())).collect();
        let complete_ctx = Context { players: &player_names, ops: &self.net.session.ops, bodies: &body_names, presets: &presets, op: self.net.session.op };
        let my_id = world.my_id;
        let alive_bodies = row.props.alive.iter().filter(|a| **a).count();
        let title = if bases && world.roll.twist != Twist::Plain {
            format!("{}   ·   ROUND {}   ·   {}", world.preset.to_uppercase(), world.round, world.roll.twist.name().to_uppercase())
        } else {
            format!("{}   ·   ROUND {}", world.preset.to_uppercase(), world.round)
        };
        let horizon = (paths.coast.len().max(1) - 1) as f64 / world.rules.tick_hz as f64;
        let predict_ms = paths.compute_ms;
        let (rate, since, made) = self.predicting;
        if now - since >= 1.0 {
            self.predicting = ((self.predictor.made - made) as f64 / (now - since), now, self.predictor.made);
        }
        let per_second = rate;
        let net_lines: Vec<String> = vec![
            format!("{} bodies   time x{:.0}   {:.0} fps   draw {:.1} ms ({})", alive_bodies, world.rules.time_scale(), self.meter.fps, self.meter.ms, if self.density.picture().2 { "GPU" } else { "CPU" }),
            format!("tick {:.1}  head {}  ephemeris +{} ticks", tick_f, world.head, world.eph.end_tick().unwrap_or(0).saturating_sub(world.head)),
            format!("rtt {:.0} ms   input lead {} ticks ({:.0} ms)", rtt.unwrap_or(0.0) * 1e3, lead, lead as f64 * 1e3 / world.rules.tick_hz as f64),
            format!("world hash: {} checks, {} mismatches, {} resyncs", stats.hash_checks, stats.hash_mismatches, stats.resyncs),
            format!("ship checks: {} ok, {} corrected", stats.ship_checks - stats.ship_corrections, stats.ship_corrections),
            format!("my inputs: {} sent, {} re-timed by server", stats.cmds_sent, stats.retimed_cmds),
            format!("remote inputs in my past: {}  (ship replays {})", stats.late_remote_inputs, stats.rollbacks),
            format!("net {:.1} kB/s up, {:.1} kB/s down", bytes.0 / 1e3, bytes.1 / 1e3),
            format!("prediction {horizon:.1} s ahead, computed in {predict_ms:.0} ms, {per_second:.0} a second"),
        ];
        let status = self.status.as_ref().filter(|s| s.1 > get_time()).map(|s| s.0.clone());
        // The ship's engine and tank, what it carries, and what is banked and bought.
        let kit = me.and_then(|m| m.ship.as_ref()).map(|t| t.last().kit);
        let fuel_max = kit.map_or(world.rules.fuel_max_mmps, |k| k.tank);
        let thrust_share = kit.map_or(1.0, |k| k.push as f32 / k.burn.max(1) as f32);
        let (cargo, bank, upgrades) = me.map_or(Default::default(), |m| (m.cargo, m.bank, m.upgrades));
        let shop_open = self.shop_open;
        let rules_now = world.rules.clone();
        let mut bought: Option<Ware> = None;
        let (items, shielded, locked_on, rapid) = me.map_or((Items::default(), false, false, false), |m| (m.items, m.shield, m.lock, m.quick));
        // Seconds of scrap magnet left.
        let magnet = me.map(|m| (m.magnet_until as f64 - tick_f) / world.rules.tick_hz as f64).filter(|s| *s > 0.0);
        let thrust_pct = self.thrust_pct;
        // The ruler replaces a "meters per pixel" readout.
        style::scale_bar(screen_width() * 0.5, screen_height() - 18.0 * hud, view.mpp, fmt::distance_round, hud);

        let mut outcome = Outcome::Continue;
        let mut menu = self.menu_open;
        let (mut has_ptr, mut has_kb) = (false, false);
        let mut chat_out = chat::Outcome::default();
        // Wheel movement in notches: raw units differ per platform, as for zooming.
        let chat_wheel = wheel * settings.zoom_speed;
        let mut options = settings.clone();
        egui_macroquad::ui(|ctx| {
            use egui::{Align2, Area, Id, RichText};
            ctx.set_zoom_factor(hud);
            let (gold, dim, good, bad) = (style::c32(style::GOLD), style::c32(style::DIM), style::c32(style::GOOD), style::c32(style::EMBER));

            // Top center: where we are and how long is left.
            Area::new(Id::new("round")).anchor(Align2::CENTER_TOP, [0.0, 10.0]).interactable(false).show(ctx, |ui| {
                // One line each, however long: wrapped, the title looks broken.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(&title).small().color(dim).extra_letter_spacing(2.0));
                    match (intermission, round_left) {
                        (None, _) if sudden_left.is_some() => {
                            let left = sudden_left.unwrap();
                            if left > 0.0 {
                                let color = if left < 60.0 { bad } else { style::c32(style::TEXT) };
                                ui.label(RichText::new(clock(left)).heading().color(color));
                                ui.label(RichText::new("until sudden death").small().color(dim));
                            } else {
                                let pulse = 0.65 + 0.35 * (now as f32 * 4.0).sin().abs();
                                ui.label(RichText::new("SUDDEN DEATH").heading().color(bad.gamma_multiply(pulse)));
                            }
                        }
                        (Some(left), _) => {
                            let verdict = match (bases, &winner, scores.first()) {
                                (true, Some(w), _) => format!("{w} wins"),
                                (true, None, _) => "Nobody won".to_string(),
                                (false, _, Some(best)) if best.score > 0 => format!("{} wins with {}", best.name, best.score),
                                _ => "Nobody scored".to_string(),
                            };
                            ui.label(RichText::new(verdict).heading().color(gold));
                            ui.label(RichText::new(format!("new world in {left:.0} s")).color(dim));
                        }
                        (None, Some(left)) => {
                            let color = if left < 30.0 { bad } else { style::c32(style::TEXT) };
                            ui.label(RichText::new(clock(left)).heading().color(color));
                        }
                        _ => {}
                    }
                    if let Some(s) = &status {
                        ui.label(RichText::new(s).color(gold));
                    }
                    for (what, value, frac, color) in &happening {
                        ui.add_space(4.0);
                        ui.allocate_ui(egui::vec2(280.0, 0.0), |ui| {
                            ui.set_width(280.0);
                            let value = value.clone().unwrap_or_else(|| format!("{:.0} %", frac.min(1.0) * 100.0));
                            style::gauge(ui, what, &value, *frac, *color);
                        });
                    }
                    for (text, color, fade) in &alerts {
                        ui.label(RichText::new(text).size(17.0).strong().color(style::c32(style::alpha(*color, *fade))));
                    }
                });
            });

            // Top left: diagnostics, only on request; the objective sits under them.
            let mut below = 10.0;
            if options.details {
                let shown = Area::new(Id::new("net")).anchor(Align2::LEFT_TOP, [10.0, 10.0]).interactable(false).show(ctx, |ui| {
                    style::panel().show(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        for l in &net_lines {
                            ui.label(RichText::new(l).monospace().color(dim));
                        }
                    });
                });
                below = shown.response.rect.bottom() + 8.0;
            }
            // Where to orbit, which conditions hold, progress.
            if let Some(name) = &target_name {
                Area::new(Id::new("objective")).fixed_pos([10.0, below]).interactable(false).show(ctx, |ui| {
                    style::panel().show(ui, |ui| {
                        ui.set_width(230.0);
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        ui.horizontal(|ui| {
                            style::caption(ui, "ORBIT");
                            ui.label(RichText::new(format!("{name}  +{capture_points}")).color(gold).strong());
                        });
                        ui.horizontal(|ui| {
                            let mark = |ui: &mut egui::Ui, what: &str, ok: Option<bool>| {
                                let color = match ok {
                                    Some(true) => good,
                                    Some(false) => bad,
                                    None => dim,
                                };
                                ui.label(RichText::new(what).small().color(color).extra_letter_spacing(1.0));
                            };
                            let state = orbit.filter(|(o, _)| o.bound);
                            mark(ui, "ECC", state.map(|(o, _)| o.ecc <= limits.0));
                            mark(ui, "LOW", state.map(|(o, r)| o.peri / r >= limits.1));
                            mark(ui, "HIGH", state.map(|(o, r)| o.apo / r <= limits.2));
                        });
                        // The time left stands still, dimmed, while somebody is on the orbit.
                        let time = |ui: &mut egui::Ui, label: &str, (left, frac, held): &(String, f32, bool)| {
                            style::gauge(ui, label, left, *frac, if *held { style::DIM } else { style::ACCENT });
                        };
                        if let Some(t) = &target_time {
                            time(ui, if t.2 { "TIME · HELD" } else { "TIME" }, t);
                        }
                        for (rival, frac, color) in &rivals {
                            style::gauge(ui, &rival.to_uppercase(), &format!("{:.0} %", frac.min(1.0) * 100.0), *frac, *color);
                        }
                        if !others.is_empty() {
                            style::section(ui, "OTHER TARGETS");
                            for (name, t) in &others {
                                if target_time.is_some() {
                                    time(ui, &name.to_uppercase(), t);
                                } else {
                                    ui.label(RichText::new(name).small());
                                }
                            }
                        }
                    });
                });
            }

            // Bases: one's own, the one being attacked, and how the siege orbit is doing.
            if bases && intermission.is_none() && (home.is_some() || siege.is_some()) {
                Area::new(Id::new("bases")).fixed_pos([10.0, below]).interactable(false).show(ctx, |ui| {
                    style::panel().show(ui, |ui| {
                        ui.set_width(230.0);
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        match home {
                            Some(h) if h > 0.0 => {
                                let color = if besiegers.is_empty() { style::GOOD } else { style::EMBER };
                                style::gauge(ui, "HOME", &format!("{:.0} %", h * 100.0), h, color);
                            }
                            Some(_) => {
                                style::caption(ui, "HOME");
                                ui.label(RichText::new("destroyed: no more ships").color(bad));
                            }
                            None => {}
                        }
                        if let Some((name, color, h, state, on)) = &siege {
                            style::section(ui, if *on { "SIEGE · ON" } else { "SIEGE" });
                            style::gauge(ui, name, &format!("{:.0} %", h * 100.0), *h, *color);
                            if let Some((o, r)) = state.filter(|(o, _)| o.bound) {
                                ui.horizontal(|ui| {
                                    let mark = |ui: &mut egui::Ui, what: &str, ok: bool| {
                                        ui.label(RichText::new(what).small().color(if ok { good } else { bad }).extra_letter_spacing(1.0));
                                    };
                                    mark(ui, "ECC", o.ecc <= limits.0);
                                    mark(ui, "LOW", o.peri / r >= limits.1);
                                    mark(ui, "HIGH", o.apo / r <= limits.2);
                                });
                            }
                        }
                    });
                });
            }

            // Top right: standings and what just happened.
            Area::new(Id::new("score")).anchor(Align2::RIGHT_TOP, [-10.0, 10.0]).interactable(false).show(ctx, |ui| {
                style::panel().show(ui, |ui| {
                    egui::Grid::new("scores").num_columns(5).spacing([14.0, 3.0]).show(ui, |ui| {
                        let heads = if bases { ["PLAYER", "BASE", "KO", "K", "D"] } else { ["PLAYER", "PTS", "ORB", "K", "D"] };
                        for head in heads {
                            style::caption(ui, head);
                        }
                        ui.end_row();
                        for row in &scores {
                            let name = RichText::new(&row.name).color(style::c32(row.color));
                            ui.label(if row.mine { name.underline() } else { name });
                            let first = match (bases, row.score) {
                                (false, s) => s.to_string(),
                                (true, hp) if hp > 0 => format!("{:.0}%", 100.0 * hp as f32 / BASE_HP as f32),
                                (true, 0) => "down".to_string(),
                                (true, _) => "-".to_string(),
                            };
                            let base_color = if teams_on { style::team_color(row.team) } else { style::TEXT };
                            ui.label(RichText::new(first).color(style::c32(base_color)).strong());
                            for n in row.counts {
                                ui.label(RichText::new(n.to_string()).color(dim));
                            }
                            ui.end_row();
                        }
                    });
                    if !watchers.is_empty() {
                        style::caption(ui, "WATCHING");
                        ui.horizontal_wrapped(|ui| {
                            for (name, color) in &watchers {
                                ui.label(RichText::new(name).color(style::c32(style::alpha(*color, 0.7))));
                            }
                        });
                    }
                });
            });

            // Bottom right: the ship.
            Area::new(Id::new("ship")).anchor(Align2::RIGHT_BOTTOM, [-10.0, -10.0]).interactable(false).show(ctx, |ui| {
                style::panel().show(ui, |ui| {
                    ui.set_width(220.0);
                    match fuel {
                        Some(f) => {
                            let frac = f as f32 / fuel_max as f32;
                            let fuel_color = if frac < 0.2 { style::EMBER } else { style::ACCENT };
                            style::gauge(ui, "DELTA-V", &fmt::speed(f as f64 / 1000.0), frac, fuel_color);
                            style::gauge(ui, "THROTTLE", &format!("{thrust_pct:.0} %"), thrust_pct / 100.0, style::GOOD);
                            if free_shells {
                                // At home shells cost nothing.
                                style::gauge(ui, "SHELLS", "free", 1.0, if reloading { style::DIM } else { style::GOOD });
                            } else {
                                let stock = (shells as f32 + next_shell) / shells_max.max(1) as f32;
                                let ready = shells > 0 && !reloading;
                                style::gauge(ui, "SHELLS", &format!("{shells} / {shells_max}"), stock, if ready { style::EMBER } else { style::DIM });
                            }
                            if !cargo.is_empty() {
                                // What it carries, by how much less the engine pushes it.
                                let what = format!("{} · thrust {:.0} %", cargo.describe(), 100.0 * thrust_share);
                                style::gauge(ui, "HOLD", &what, thrust_share, style::piece_color(PieceKind::Scrap));
                            }
                            // What is on the ship for now: buffs, with no key to use them.
                            for (on, what) in [(shielded, "SHIELD"), (rapid, "RAPID FIRE"), (locked_on, "TARGET LOCK")] {
                                if on {
                                    style::row(ui, what, RichText::new("on").color(good), None);
                                }
                            }
                            if let Some(left) = magnet {
                                style::row(ui, "SCRAP MAGNET", RichText::new(format!("{left:.0} s left")).color(good), None);
                            }
                            // Supplies carried, and the keys that use them.
                            for (k, supply) in Supply::CARRIED.into_iter().enumerate() {
                                let n = items.count(supply);
                                if n > 0 {
                                    let key = (k + 1).to_string();
                                    style::row(ui, &supply.name().to_uppercase(), RichText::new(n.to_string()).color(gold), Some(&key));
                                }
                            }
                        }
                        None if spectating => {
                            style::caption(ui, if out { "OUT · WATCHING" } else { "WATCHING" });
                            ui.label(RichText::new("Space: next player").color(dim));
                        }
                        None => {
                            style::caption(ui, "SHIP LOST");
                            if respawn_in.is_none() {
                                ui.label(RichText::new("waiting for a ship").heading().color(bad));
                            }
                        }
                    }
                    if bases && playing {
                        // What is banked, and the key for the shop where it is open.
                        let value = RichText::new(format!("{} scrap · {} crystals", bank.scrap, bank.crystals)).color(style::c32(style::piece_color(PieceKind::Scrap)));
                        style::row(ui, "BANK", value, can_shop.then_some("B"));
                    }
                });
            });

            // Center: a ring that fills while the self-destruct key is held, or until the
            // new ship arrives. (what it says, how full it is, seconds left)
            let countdown = match (destruct, respawn_in) {
                (Some(frac), _) => Some(("SELF-DESTRUCT", frac, (1.0 - frac as f64) * DESTRUCT_HOLD)),
                (None, Some(left)) => Some(("RESPAWN", 1.0 - (left / respawn_secs) as f32, left)),
                _ => None,
            };
            if let Some((what, frac, left)) = countdown {
                Area::new(Id::new("countdown")).anchor(Align2::CENTER_CENTER, [0.0, 100.0]).interactable(false).show(ctx, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.vertical_centered(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(66.0, 66.0), egui::Sense::hover());
                        let (c, r) = (rect.center(), 28.0);
                        ui.painter().circle_stroke(c, r, egui::Stroke::new(5.0, egui::Color32::from_rgb(24, 32, 46)));
                        // Clockwise from the top.
                        let arc: Vec<egui::Pos2> = (0..=64)
                            .map(|k| k as f32 / 64.0 * frac.clamp(0.0, 1.0) * std::f32::consts::TAU)
                            .map(|a| c + r * egui::vec2(a.sin(), -a.cos()))
                            .collect();
                        ui.painter().add(egui::Shape::line(arc, egui::Stroke::new(5.0, bad)));
                        ui.painter().text(c, Align2::CENTER_CENTER, format!("{:.1}", left.max(0.0)), egui::FontId::proportional(19.0), bad);
                        ui.label(RichText::new(what).color(bad).strong().extra_letter_spacing(2.5));
                    });
                });
            }

            if shop_open {
                bought = shop(ctx, &rules_now, &bank, &upgrades, &items, (shielded, locked_on, rapid));
            }

            chat_out = self.chat.show(ctx, &self.net.session.chat, now, chat_wheel, &complete_ctx, &mentions);
            if menu {
                egui::Window::new("MENU").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
                    ctx,
                    |ui| {
                        ui.set_width(330.0);
                        crate::options::scrolled(ui, |ui| crate::options::show(ui, &mut options, crate::options::World::Exact));
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
        if let Some(ware) = bought {
            self.net.session.buy(ware);
        }
        match chat_out.clicked {
            Some(Mention::Body(slot)) => {
                self.selected = Some(slot);
                self.picked = None;
                self.watch = None;
            }
            Some(Mention::Player(id)) => self.watch = Some(id).filter(|id| *id != my_id),
            None => {}
        }
        // Changed in the menu just now, or by a key earlier in the frame.
        if options != before {
            *settings = options;
            self.settings_dirty = true;
        } else if self.settings_dirty && !is_mouse_button_down(MouseButton::Left) {
            self.settings_dirty = false;
            settings.save();
        }
        self.menu_open = menu;
        self.ui_has_pointer = has_ptr;
        self.ui_has_keyboard = has_kb;
        self.busy += 0.1 * (fmt::now() - began - predicting - self.busy);
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
        // Cursor distance reads as "meters per real second".
        (math::dir_to_angle(dx, dy), dist / scale)
    }
}


/// How long a merge flash lasts, in ticks.
const FLASH_TICKS: f64 = 36.0;

/// The automatic camera: it leaves the view alone while everything fits, and eases to a new
/// one when something nears the edge or everything has been small for a while.
struct Auto {
    /// On its way to a new view.
    moving: bool,
    small_since: Option<f64>,
    /// The box around the predicted path, and when it was last taken.
    path: Option<Bounds>,
    path_at: f64,
    /// The player moved the view by hand: leave it be until then.
    hands_off_until: f64,
    /// What it last framed (selection and round): a change is a reason to move at once.
    framed: Option<(Option<RefKey>, Option<u32>)>,
}

/// Part of the screen, from the middle to the edge, that a new view fills.
const AUTO_FILL: f64 = 0.6;
/// Reaching this far towards the edge calls for a new view.
const AUTO_EDGE: f64 = 0.9;
/// So does filling less than this of the screen for [`AUTO_PATIENCE`] seconds.
const AUTO_SMALL: f64 = 0.25;
const AUTO_PATIENCE: f64 = 1.5;
/// Seconds between looks at the predicted path.
const AUTO_PATH_EVERY: f64 = 2.0;
/// Seconds after a burn before the path it left is looked at.
const AUTO_AFTER_BURN: f64 = 0.5;
/// Seconds a view moved by hand is left alone.
const AUTO_HANDS_OFF: f64 = 3.0;
/// How quickly it eases over, per second.
const AUTO_RATE: f64 = 3.0;

impl Auto {
    fn steer(&mut self, seen: &Bounds, mpp: &mut f64, offset: &mut (f64, f64), fresh: bool, thrusting: bool, dt: f64) {
        // A new subject is framed as soon as the engine allows.
        self.moving |= fresh;
        // The view holds still under thrust: flying needs a steady picture.
        if thrusting {
            self.small_since = None;
            return;
        }
        let (sw, sh) = (screen_width() as f64, screen_height() as f64);
        let (w, h) = (seen.max.0 - seen.min.0, seen.max.1 - seen.min.1);
        if !(w.max(h) > 0.0 && w.is_finite() && h.is_finite()) {
            return;
        }
        let base = *mpp;
        let reach_x = (seen.max.0 - offset.0).max(offset.0 - seen.min.0) / (0.5 * sw * base);
        let reach_y = (seen.max.1 - offset.1).max(offset.1 - seen.min.1) / (0.5 * sh * base);
        let fill = (w / (sw * base)).max(h / (sh * base));
        let now = get_time();
        if now < self.hands_off_until && !fresh {
            self.small_since = None;
            return;
        }
        if fill < AUTO_SMALL {
            self.small_since.get_or_insert(now);
        } else {
            self.small_since = None;
        }
        if fresh || reach_x.max(reach_y) > AUTO_EDGE || self.small_since.is_some_and(|t| now - t > AUTO_PATIENCE) {
            self.moving = true;
        }
        if !self.moving {
            return;
        }
        let middle = (0.5 * (seen.min.0 + seen.max.0), 0.5 * (seen.min.1 + seen.max.1));
        let goal = ((w / sw).max(h / sh) / AUTO_FILL).clamp(MIN_MPP, 1.0e12);
        let k = 1.0 - (-AUTO_RATE * dt).exp();
        // The scale eases over, and the center goes as far of its way as the scale has gone of
        // its own: that is a zoom about one fixed point, in which every edge of the view moves
        // straight from where it is to where it will be. Nothing that is in view before and
        // after is out of it in between.
        let was = *mpp;
        *mpp *= (goal / was).powf(k);
        let part = if (goal - was).abs() > 1.0e-9 * was { (*mpp - was) / (goal - was) } else { k };
        offset.0 += (middle.0 - offset.0) * part;
        offset.1 += (middle.1 - offset.1) * part;
        let off = ((middle.0 - offset.0).abs() / sw).max((middle.1 - offset.1).abs() / sh) / *mpp;
        if off < 0.005 && (goal / *mpp).ln().abs() < 0.01 {
            self.moving = false;
            self.small_since = None;
        }
    }
}

/// A box around points and discs.
#[derive(Clone, Copy)]
struct Bounds {
    min: (f64, f64),
    max: (f64, f64),
}

impl Default for Bounds {
    fn default() -> Self {
        Self { min: (f64::MAX, f64::MAX), max: (f64::MIN, f64::MIN) }
    }
}

impl Bounds {
    fn add(&mut self, x: f64, y: f64, r: f64) {
        if x.is_finite() && y.is_finite() {
            self.min = (self.min.0.min(x - r), self.min.1.min(y - r));
            self.max = (self.max.0.max(x + r), self.max.1.max(y + r));
        }
    }
}

/// A new ship has appeared while the camera follows a body: unless both are already on
/// screen, center the view between them and zoom so that both are.
pub(crate) fn frame_both(view: &mut View, offset: &mut (f64, f64), body: (f64, f64), ship: (f64, f64)) {
    (view.cx, view.cy) = (body.0 + offset.0, body.1 + offset.1);
    if view.on_screen(view.to_screen(body.0, body.1), 0.0) && view.on_screen(view.to_screen(ship.0, ship.1), 0.0) {
        return;
    }
    fit_both(view, offset, body, ship);
}

/// Look at `body` (what the camera stays with) and `ship` together, whatever the view was.
pub(crate) fn fit_both(view: &mut View, offset: &mut (f64, f64), body: (f64, f64), ship: (f64, f64)) {
    let (dx, dy) = (ship.0 - body.0, ship.1 - body.1);
    *offset = (0.5 * dx, 0.5 * dy);
    // Each of them half way from the middle to the edge on the tighter axis: clear of the
    // panels in the corners, and with room for the ship to move.
    let needed = (dx.abs() / (0.5 * screen_width() as f64)).max(dy.abs() / (0.5 * screen_height() as f64));
    view.mpp = needed.clamp(MIN_MPP, 1.0e12);
}

/// A predicted path, fading out towards its end (the further ahead, the less it is to be relied on).
pub(crate) fn draw_path(points: &[(f64, f64)], anchor: (f64, f64), view: &View, ui: f32, color: Color) {
    let mut prev: Option<(f32, f32)> = None;
    let n = points.len().max(1) as f32;
    for (k, q) in points.iter().enumerate() {
        let s = view.to_screen(anchor.0 + q.0, anchor.1 + q.1);
        if let Some(p) = prev {
            if (view.on_screen(p, 200.0) || view.on_screen(s, 200.0)) && s.0.is_finite() && s.1.is_finite() {
                let fade = 1.0 - 0.85 * k as f32 / n;
                draw_line(p.0, p.1, s.0, s.1, 1.5 * ui, style::alpha(color, color.a * fade));
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
/// An arrow on the edge of the screen towards the off-screen point `s`. Returns where a
/// label for it goes.
fn edge_arrow(s: (f32, f32), color: Color, ui: f32) -> (f32, f32) {
    let (cx, cy) = (screen_width() * 0.5, screen_height() * 0.5);
    let (dx, dy) = (s.0 - cx, s.1 - cy);
    let k = ((cx - 24.0 * ui) / dx.abs().max(1e-3)).min((cy - 24.0 * ui) / dy.abs().max(1e-3));
    let (ex, ey) = (cx + dx * k, cy + dy * k);
    let a = dy.atan2(dx);
    let tip = |fwd: f32, side: f32| vec2(ex + a.cos() * fwd - a.sin() * side, ey + a.sin() * fwd + a.cos() * side);
    draw_triangle(tip(10.0 * ui, 0.0), tip(-6.0 * ui, 7.0 * ui), tip(-6.0 * ui, -7.0 * ui), color);
    (ex - a.cos() * 34.0 * ui, ey - a.sin() * 34.0 * ui + 4.0 * ui)
}

pub(crate) fn draw_ship(s: (f32, f32), facing: f32, color: Color, burning: bool, ui: f32) {
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
    // Where the ship really is: the point shells have to reach.
    style::disc(s.0, s.1, 2.8 * ui, Color::new(0.0, 0.0, 0.0, 0.9));
    style::disc(s.0, s.1, 1.7 * ui, WHITE);
}


impl Game {
    /// Tell the server we are going, so our name and ship are released at once.
    pub fn leave(&mut self) {
        self.net.disconnect();
    }
}

/// The shop, at home: the next tier of each upgrade, and supplies, for what is banked.
/// Returns what was bought, if anything.
fn shop(ctx: &egui::Context, rules: &gsim_core::GameRules, bank: &Purse, upgrades: &Upgrades, items: &Items, (shield, lock, quick): (bool, bool, bool)) -> Option<Ware> {
    use egui::RichText;
    let mut bought = None;
    let (dim, gold, text) = (style::c32(style::DIM), style::c32(style::GOLD), style::c32(style::TEXT));
    let (scrap, crystal) = (style::c32(style::piece_color(PieceKind::Scrap)), style::c32(style::piece_color(PieceKind::Crystal)));
    let price_button = |ui: &mut egui::Ui, price: Purse, can: bool| {
        let label = RichText::new(price.describe()).color(if price.crystals > 0 { crystal } else { scrap });
        ui.add_enabled(can && bank.covers(price), egui::Button::new(label)).clicked()
    };
    egui::Window::new("SHOP").collapsible(false).resizable(false).anchor(egui::Align2::LEFT_CENTER, [10.0, 0.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            style::caption(ui, "BANK");
            ui.label(RichText::new(format!("{} scrap", bank.scrap)).color(scrap).strong());
            ui.label(RichText::new(format!("{} crystals", bank.crystals)).color(crystal).strong());
        });
        egui::Grid::new("shop").num_columns(7).spacing([10.0, 5.0]).show(ui, |ui| {
            let ups = Upgrade::SHIP.into_iter().chain(Upgrade::BASE);
            for (k, u) in ups.enumerate() {
                if k == 0 || k == Upgrade::SHIP.len() {
                    for head in ["", if k == 0 { "SHIP" } else { "BASE" }, "", "NOW", "NEXT", ""] {
                        style::caption(ui, head);
                    }
                    ui.end_row();
                }
                let tier = upgrades.tier(u);
                style::key_cap(ui, &(k + 1).to_string());
                ui.label(RichText::new(u.name().to_uppercase()).strong());
                // Tiers bought, out of all of them.
                let (rect, _) = ui.allocate_exact_size(egui::vec2(TIERS as f32 * 11.0, 10.0), egui::Sense::hover());
                for t in 0..TIERS {
                    let pip = egui::Rect::from_min_size(rect.min + egui::vec2(t as f32 * 11.0, 1.0), egui::vec2(8.0, 8.0));
                    if t < tier {
                        ui.painter().rect_filled(pip, 1.0, gold);
                    } else {
                        ui.painter().rect_stroke(pip, 1.0, egui::Stroke::new(1.0, dim), egui::StrokeKind::Inside);
                    }
                }
                ui.label(RichText::new(u.tier_text(rules, tier)).color(dim));
                if tier < TIERS {
                    ui.label(RichText::new(u.tier_text(rules, tier + 1)).color(text));
                    if price_button(ui, u.price(tier + 1), true) {
                        bought = Some(Ware::Upgrade(u));
                    }
                } else {
                    ui.label("");
                    ui.label(RichText::new("MAX").color(gold));
                }
                ui.end_row();
            }
        });
        egui::Grid::new("supplies").num_columns(3).spacing([10.0, 5.0]).show(ui, |ui| {
            style::caption(ui, "SUPPLIES");
            ui.end_row();
            for s in Supply::ALL {
                ui.label(RichText::new(s.name().to_uppercase()).strong());
                let have = match s {
                    Supply::Shield if shield => "on".to_string(),
                    Supply::Lock if lock => "on".to_string(),
                    Supply::RapidFire if quick => "on".to_string(),
                    _ if s.carried() => format!("{} / {}", items.count(s), shop::CARRY),
                    _ => String::new(),
                };
                ui.label(RichText::new(have).color(gold));
                let can = match s {
                    Supply::Shield => !shield,
                    Supply::Lock => !lock,
                    Supply::RapidFire => !quick,
                    _ => !s.carried() || items.count(s) < shop::CARRY,
                };
                if price_button(ui, s.price(), can) {
                    bought = Some(Ware::Supply(s));
                }
                ui.end_row();
            }
        });
    });
    bought
}

/// A line of the scoreboard.
struct ScoreRow {
    name: String,
    /// Points, or with bases the health of the player's base (-1 for none).
    score: i64,
    /// Captures (with bases: bases destroyed), kills, deaths.
    counts: [u32; 3],
    mine: bool,
    color: Color,
    team: TeamId,
}
