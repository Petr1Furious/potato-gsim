//! Local replica of the game world. Massive bodies come from the ephemeris; every ship and
//! shell has its own history, so a late input or correction re-simulates only that entity.

use crate::eph::Eph;
use crate::session::Stats;
use gsim_core::particle::step_particle;
use gsim_core::ship::{step_ship, wells_at};
use gsim_core::{math, EphRow, GameRules, InputTimeline, Kit, Magazine, Particle, Scratch, ShipState, Tick};
use gsim_proto::shop;
use gsim_proto::*;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

/// How many ticks of history each entity keeps for rollback.
pub const HISTORY_TICKS: usize = 240;

/// Per-entity state history. `hist[k]` is the state at the start of tick `base + k`.
pub struct Track<S> {
    pub base: Tick,
    pub hist: VecDeque<S>,
    /// Earliest tick whose state is still trusted; everything after it must be re-simulated.
    pub dirty_from: Option<Tick>,
    /// Locally predicted crash into this body slot; awaiting the server's verdict.
    pub crashed: Option<u32>,
    /// Server-confirmed end of life: the entity does not exist from this tick on.
    pub dies_at: Option<Tick>,
}

impl<S: Copy> Track<S> {
    pub fn new(tick: Tick, state: S) -> Self {
        Self { base: tick, hist: VecDeque::from([state]), dirty_from: None, crashed: None, dies_at: None }
    }

    /// Tick of the newest state.
    pub fn end(&self) -> Tick {
        self.base + self.hist.len() as Tick - 1
    }

    pub fn at(&self, tick: Tick) -> Option<&S> {
        tick.checked_sub(self.base).and_then(|i| self.hist.get(i as usize))
    }

    pub fn last(&self) -> &S {
        self.hist.back().expect("track is never empty")
    }

    pub fn mark_dirty(&mut self, tick: Tick) {
        if tick < self.end() {
            let t = tick.max(self.base);
            self.dirty_from = Some(self.dirty_from.map_or(t, |d| d.min(t)));
        }
    }

    /// Replace the state at `tick` with an authoritative one and schedule a replay from there.
    /// Returns true if it differed.
    pub fn correct(&mut self, tick: Tick, state: S) -> bool
    where
        S: PartialEq,
    {
        let Some(i) = tick.checked_sub(self.base).map(|i| i as usize) else { return false };
        match self.hist.get_mut(i) {
            Some(s) if *s != state => {
                *s = state;
                // Force a replay even when the corrected state is the newest one.
                self.hist.truncate(i + 1);
                self.crashed = None;
                self.dirty_from = None;
                true
            }
            _ => false,
        }
    }

    /// Roll back if needed, then step forward to `target`. Returns true if a rollback happened.
    fn advance(&mut self, target: Tick, mut step: impl FnMut(&mut S, Tick) -> Option<Option<u32>>) -> bool {
        let mut rolled = false;
        if let Some(d) = self.dirty_from.take() {
            let keep = (d.saturating_sub(self.base) as usize + 1).min(self.hist.len());
            if keep < self.hist.len() {
                self.hist.truncate(keep);
                self.crashed = None;
                rolled = true;
            }
        }
        let limit = self.dies_at.map_or(target, |d| d.min(target));
        while self.crashed.is_none() && self.end() < limit {
            let t = self.end();
            let mut s = *self.last();
            let Some(hit) = step(&mut s, t) else { break };
            self.hist.push_back(s);
            self.crashed = hit;
        }
        while self.hist.len() > HISTORY_TICKS {
            self.hist.pop_front();
            self.base += 1;
        }
        rolled
    }
}

/// Smooth position/velocity between two tick states (cubic Hermite).
fn hermite(a: &Particle, b: &Particle, u: f64, dt: f64) -> Particle {
    let (u2, u3) = (u * u, u * u * u);
    let (h00, h10, h01, h11) = (2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, -2.0 * u3 + 3.0 * u2, u3 - u2);
    Particle {
        x: h00 * a.x + h10 * dt * a.vx + h01 * b.x + h11 * dt * b.vx,
        y: h00 * a.y + h10 * dt * a.vy + h01 * b.y + h11 * dt * b.vy,
        vx: a.vx + (b.vx - a.vx) * u,
        vy: a.vy + (b.vy - a.vy) * u,
    }
}

fn sample<S: Copy>(track: &Track<S>, tick_f: f64, p: impl Fn(&S) -> Particle, dt: f64) -> Option<Particle> {
    if tick_f < track.base as f64 {
        return None;
    }
    let t0 = tick_f.floor() as Tick;
    let a = match track.at(t0) {
        Some(a) => p(a),
        None => return Some(p(track.last())),
    };
    match track.at(t0 + 1) {
        Some(b) => Some(hermite(&a, &p(b), tick_f - t0 as f64, dt)),
        None => Some(a),
    }
}

pub struct PlayerRep {
    pub name: String,
    pub color: [u8; 3],
    pub kills: u32,
    pub deaths: u32,
    pub captures: u32,
    pub timeline: InputTimeline,
    pub ship: Option<Track<ShipState>>,
    pub respawn_tick: Option<Tick>,
    pub next_fire_tick: Tick,
    pub magazine: Magazine,
    /// Watching, with no ship.
    pub spectating: bool,
    /// The ship survives one shell.
    pub shield: bool,
    /// Fires and reloads twice as fast, with twice the shells (from ammo, until the ship is lost).
    pub quick: bool,
    /// What the ship carries, what is banked at home, and what has been bought.
    pub cargo: Purse,
    pub bank: Purse,
    pub upgrades: Upgrades,
    pub items: Items,
    /// The ship's engine and tank from each of these ticks on.
    pub kits: BTreeMap<Tick, Kit>,
}

/// A piece of scrap or a crystal, floating free.
pub struct PieceRep {
    pub kind: PieceKind,
    pub track: Track<Particle>,
}

pub struct ShellRep {
    pub owner: PlayerId,
    pub spawn_tick: Tick,
    pub track: Track<Particle>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectKind {
    ShipDestroyed,
    ShellBlast,
    /// Somebody completed the orbit objective around the body at this spot.
    Captured,
    /// A base went down here.
    BaseDestroyed,
    /// Somebody took a crystal from a mine.
    Mined,
    /// A ship jumped home from here.
    Warped,
    /// A ship's shield took a shell.
    ShieldBroken,
    /// Somebody took a piece of scrap or a crystal.
    Pickup(PieceKind),
}

/// Something to draw for a moment (drifts with the velocity it had).
#[derive(Clone, Copy, Debug)]
pub struct Effect {
    pub tick: Tick,
    pub at: Particle,
    pub kind: EffectKind,
}

/// Something that just happened to us, for the top of the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub tick: Tick,
    pub text: String,
    /// Good news (in our favour) or bad.
    pub good: bool,
}

pub struct World {
    pub rules: GameRules,
    pub my_id: PlayerId,
    pub mode: GameMode,
    pub preset: String,
    pub names: BTreeMap<u32, String>,
    pub eph: Eph,
    pub players: BTreeMap<PlayerId, PlayerRep>,
    pub shells: BTreeMap<ShellId, ShellRep>,
    pub pieces: BTreeMap<PieceId, PieceRep>,
    /// Every entity has been simulated up to the start of this tick (ephemeris permitting).
    pub head: Tick,
    pub effects: VecDeque<Effect>,
    pub feed: VecDeque<(Tick, String)>,
    pub round: u32,
    pub round_end_tick: Option<Tick>,
    /// Set between rounds.
    pub next_round_tick: Option<Tick>,
    /// The bodies to orbit for points, with the time each has left as last heard.
    pub objectives: Vec<Objective>,
    /// The objective each player is on a qualifying orbit around, and for how many ticks (from
    /// the server, a few times a second).
    pub holds: BTreeMap<PlayerId, (u32, u32)>,
    /// Every base, with its health as last heard.
    pub bases: Vec<Base>,
    /// The bodies that give something to whoever holds an orbit around them.
    pub mines: Vec<Mine>,
    /// When crystals next turn up near the centre.
    pub crystals_due: Option<Tick>,
    /// Gravity wells: (tick dropped, x, y).
    pub wells: Vec<(Tick, f64, f64)>,
    /// (player, base owner) for every ship on a siege orbit.
    pub sieges: Vec<(PlayerId, PlayerId)>,
    /// The last player in play, once the round is over.
    pub winner: Option<PlayerId>,
    /// What just happened to us, newest last.
    pub alerts: VecDeque<Alert>,
    scratch: Scratch,
}

impl World {
    pub fn from_welcome(w: &Welcome, eph: crate::eph::Source) -> Self {
        let tick = w.massive.tick;
        let players = w
            .players
            .iter()
            .map(|p| {
                let rep = PlayerRep {
                    name: p.name.clone(),
                    color: p.color,
                    kills: p.kills,
                    deaths: p.deaths,
                    captures: p.captures,
                    timeline: InputTimeline::from_changes(&p.inputs),
                    ship: p.ship.map(|s| Track::new(tick, s)),
                    respawn_tick: p.respawn_tick,
                    next_fire_tick: p.next_fire_tick,
                    magazine: p.magazine,
                    spectating: p.spectating,
                    shield: p.shield,
                    quick: false,
                    cargo: p.cargo,
                    bank: p.bank,
                    upgrades: p.upgrades,
                    items: p.items,
                    kits: BTreeMap::new(),
                };
                (p.id, rep)
            })
            .collect();
        let shells = w
            .shells
            .iter()
            .map(|s| (s.id, ShellRep { owner: s.owner, spawn_tick: s.spawn_tick, track: Track::new(tick, s.p) }))
            .collect();
        let pieces = w.pieces.iter().map(|p| (p.id, PieceRep { kind: p.kind, track: Track::new(tick, p.p) })).collect();
        Self {
            rules: w.rules.clone(),
            my_id: w.your_id,
            mode: w.mode,
            preset: w.preset.clone(),
            names: w.names.iter().cloned().collect(),
            eph: Eph::new(&w.massive, &w.rules, eph),
            players,
            shells,
            pieces,
            head: tick,
            effects: VecDeque::new(),
            feed: VecDeque::new(),
            scratch: Scratch::default(),
            round: w.round,
            round_end_tick: w.round_end_tick,
            next_round_tick: w.next_round_tick,
            objectives: w.objectives.clone(),
            holds: BTreeMap::new(),
            bases: w.bases.clone(),
            mines: w.mines.clone(),
            crystals_due: w.crystals_due,
            wells: w.wells.clone(),
            sieges: Vec::new(),
            winner: None,
            alerts: VecDeque::new(),
        }
    }

    pub fn me(&self) -> Option<&PlayerRep> {
        self.players.get(&self.my_id)
    }

    pub fn my_ship(&self) -> Option<&Track<ShipState>> {
        self.me().and_then(|p| p.ship.as_ref())
    }

    pub fn player_name(&self, id: PlayerId) -> &str {
        self.players.get(&id).map_or("?", |p| p.name.as_str())
    }

    pub fn is_target(&self, slot: u32) -> bool {
        self.objectives.iter().any(|o| o.slot == slot)
    }

    pub fn base_of(&self, player: PlayerId) -> Option<&Base> {
        self.bases.iter().find(|b| b.owner == player)
    }

    /// The player's ship (as far as simulated) is within their own standing base's zone, where
    /// shells cost nothing (the server decides; this is for the controls and the HUD).
    pub fn at_home(&self, player: PlayerId) -> bool {
        let Some(b) = self.base_of(player).filter(|b| b.hp > 0) else { return false };
        let Some(p) = self.players.get(&player) else { return false };
        let Some(ship) = p.ship.as_ref() else { return false };
        let Some(row) = self.eph.get(ship.end()).or_else(|| self.eph.get(self.head.saturating_sub(1))) else { return false };
        let (j, s) = (b.slot as usize, ship.last().p);
        let zone = shop::zone_radii(&p.upgrades) * row.props.radius[j];
        j < row.x.len() && (s.x - row.x[j]).powi(2) + (s.y - row.y[j]).powi(2) <= zone * zone
    }

    pub fn base_on(&self, slot: u32) -> Option<&Base> {
        self.bases.iter().find(|b| b.slot == slot)
    }

    pub fn body_name(&self, slot: u32) -> String {
        self.names.get(&slot).cloned().unwrap_or_else(|| format!("B{slot}"))
    }

    /// Tell the player about something that concerns them (the same news again only
    /// freshens it).
    pub fn alert(&mut self, tick: Tick, text: String, good: bool) {
        self.alerts.retain(|a| a.text != text);
        self.alerts.push_back(Alert { tick, text, good });
        while self.alerts.len() > 4 {
            self.alerts.pop_front();
        }
    }

    fn say(&mut self, tick: Tick, text: String) {
        self.feed.push_back((tick, text));
        while self.feed.len() > 8 {
            self.feed.pop_front();
        }
    }

    /// Apply a server event. Events may refer to ticks in our past (they then trigger a
    /// replay of the affected entity only) or in our future.
    pub fn apply_event(&mut self, e: Event, stats: &mut Stats) {
        match e {
            Event::PlayerJoined { id, name, color } => {
                self.say(self.head, format!("{name} joined the game"));
                self.players.entry(id).or_insert(PlayerRep {
                    name,
                    color,
                    kills: 0,
                    deaths: 0,
                    captures: 0,
                    timeline: InputTimeline::new(),
                    ship: None,
                    respawn_tick: None,
                    next_fire_tick: 0,
                    magazine: Magazine::full(self.head, &self.rules),
                    spectating: false,
                    shield: false,
                    quick: false,
                    cargo: Purse::default(),
                    bank: Purse::default(),
                    upgrades: Upgrades::default(),
                    items: Items::default(),
                    kits: BTreeMap::new(),
                });
            }
            Event::PlayerColor { id, color } => {
                if let Some(p) = self.players.get_mut(&id) {
                    p.color = color;
                }
            }
            Event::PlayerLeft { id } => {
                if let Some(p) = self.players.remove(&id) {
                    self.say(self.head, format!("{} left the game", p.name));
                }
            }
            Event::Gamemode { tick, player, spectating } => {
                let name = self.player_name(player).to_string();
                let out = spectating && self.mode == GameMode::Bases && self.base_of(player).is_some_and(|b| b.hp == 0);
                self.say(tick, format!("{name} {}", if out { "is out" } else if spectating { "is watching" } else { "is playing again" }));
                if out && player == self.my_id {
                    self.alert(tick, "You are out".to_string(), false);
                }
                if let Some(p) = self.players.get_mut(&player) {
                    p.spectating = spectating;
                    if spectating {
                        p.shield = false;
                        p.respawn_tick = None;
                        if let Some(ship) = p.ship.as_mut() {
                            ship.dies_at = Some(tick);
                        }
                    }
                }
            }
            Event::ShipSpawn { tick, player, state } => {
                if let Some(p) = self.players.get_mut(&player) {
                    p.ship = Some(Track::new(tick, state));
                    p.kits = BTreeMap::from([(tick, state.kit)]);
                    p.respawn_tick = None;
                    // A new ship comes fully armed, with an empty hold (or as it was placed).
                    p.magazine = shop::magazine(tick, &self.rules, &p.upgrades, p.quick);
                }
            }
            Event::Input { player, tick, input } => {
                // Our own inputs are already in our timeline; `CmdAck` handles re-timing.
                if player == self.my_id {
                    return;
                }
                if let Some(p) = self.players.get_mut(&player) {
                    p.timeline.set(tick, input);
                    if let Some(ship) = p.ship.as_mut() {
                        if tick < ship.end() {
                            stats.late_remote_inputs += 1;
                        }
                        ship.mark_dirty(tick);
                    }
                }
            }
            Event::ShellSpawn { tick, id, owner, p, free } => {
                self.shells.insert(id, ShellRep { owner, spawn_tick: tick, track: Track::new(tick, p) });
                if let Some(pl) = self.players.get_mut(&owner) {
                    pl.next_fire_tick = tick + pl.magazine.cooldown();
                    if !free {
                        pl.magazine.fire(tick, &self.rules);
                    }
                }
            }
            Event::ShellGone { tick, id, exploded } => {
                if let Some(s) = self.shells.get_mut(&id) {
                    s.track.dies_at = Some(tick);
                    if exploded {
                        let at = *s.track.at(tick).unwrap_or(s.track.last());
                        self.effects.push_back(Effect { tick, at, kind: EffectKind::ShellBlast });
                    }
                }
            }
            Event::ShipDied { tick, player, killer, body, respawn_tick } => {
                let victim = self.player_name(player).to_string();
                let text = match (killer, body) {
                    (Some(k), _) if k == player => format!("{victim} blew themselves up"),
                    (Some(k), _) => format!("{} destroyed {victim}", self.player_name(k)),
                    (None, Some(b)) => format!("{victim} crashed into {}", self.body_name(b)),
                    _ => format!("{victim} was destroyed"),
                };
                self.say(tick, text);
                let me = self.my_id;
                match (killer, body) {
                    (Some(k), _) if player == me && k != me => self.alert(tick, format!("{} destroyed your ship", self.player_name(k)), false),
                    (None, Some(b)) if player == me => self.alert(tick, format!("You crashed into {}", self.body_name(b)), false),
                    (Some(k), _) if k == me && player != me => self.alert(tick, format!("You destroyed {victim}"), true),
                    _ => {}
                }
                let counts = self.next_round_tick.is_none();
                if let Some(k) = killer.filter(|k| *k != player && counts) {
                    if let Some(p) = self.players.get_mut(&k) {
                        p.kills += 1;
                    }
                }
                if let Some(p) = self.players.get_mut(&player) {
                    p.deaths += counts as u32;
                    p.shield = false;
                    p.quick = false;
                    p.respawn_tick = Some(respawn_tick);
                    if let Some(ship) = p.ship.as_mut() {
                        ship.dies_at = Some(tick);
                        let at = ship.at(tick).unwrap_or(ship.last()).p;
                        self.effects.push_back(Effect { tick, at, kind: EffectKind::ShipDestroyed });
                    }
                }
            }
            Event::Score { player, kills, deaths, captures } => {
                if let Some(p) = self.players.get_mut(&player) {
                    (p.kills, p.deaths, p.captures) = (kills, deaths, captures);
                }
            }
            Event::RoundClock { round_end_tick } => {
                self.round_end_tick = round_end_tick;
                self.next_round_tick = None;
            }
            Event::Objectives { tick, objectives } => {
                for o in &objectives {
                    if !self.is_target(o.slot) {
                        self.say(tick, format!("New target: {}", self.body_name(o.slot)));
                    }
                }
                self.objectives = objectives;
                let slots: Vec<u32> = self.objectives.iter().map(|o| o.slot).collect();
                self.holds.retain(|_, hold| slots.contains(&hold.0));
            }
            Event::Captured { tick, player, target } => {
                self.say(tick, format!("{} captured {}", self.player_name(player), self.body_name(target)));
                if player == self.my_id {
                    self.alert(tick, format!("You captured {}", self.body_name(target)), true);
                } else if self.holds.get(&self.my_id).is_some_and(|h| h.0 == target) {
                    self.alert(tick, format!("{} captured it first", self.player_name(player)), false);
                }
                // Mark the spot (the event can be a few ticks old; the nearest row will do).
                if let Some(row) = self.eph.get(tick).or_else(|| self.eph.get(self.head.saturating_sub(1))) {
                    let j = target as usize;
                    if j < row.x.len() {
                        let at = Particle { x: row.x[j], y: row.y[j], vx: row.vx[j], vy: row.vy[j] };
                        self.effects.push_back(Effect { tick, at, kind: EffectKind::Captured });
                    }
                }
                if let Some(p) = self.players.get_mut(&player) {
                    p.captures += 1;
                }
            }
            Event::Mines { mines, .. } => {
                self.mines = mines;
                let slots: Vec<u32> = self.mines.iter().map(|s| s.slot).collect();
                self.holds.retain(|_, hold| slots.contains(&hold.0));
            }
            Event::Mined { tick, player, slot } => {
                if player == self.my_id {
                    self.alert(tick, "+1 crystal".to_string(), true);
                }
                if let Some(row) = self.eph.get(tick).or_else(|| self.eph.get(self.head.saturating_sub(1))).filter(|r| (slot as usize) < r.x.len()) {
                    let j = slot as usize;
                    let at = Particle { x: row.x[j], y: row.y[j], vx: row.vx[j], vy: row.vy[j] };
                    self.effects.push_back(Effect { tick, at, kind: EffectKind::Mined });
                }
            }
            Event::CrystalsDue { due } => {
                if self.crystals_due.is_some_and(|d| d <= self.head) {
                    self.say(self.head, "Crystals turned up near the centre".to_string());
                    self.alert(self.head, "Crystals at the centre".to_string(), true);
                }
                self.crystals_due = due;
            }
            Event::Well { tick, player, x, y } => {
                self.say(tick, format!("{} dropped a gravity well", self.player_name(player)));
                self.wells.push((tick, x, y));
                // Every ship feels it from then on.
                for p in self.players.values_mut() {
                    if let Some(ship) = p.ship.as_mut() {
                        ship.mark_dirty(tick);
                    }
                }
            }
            Event::Warped { tick, player } => {
                self.say(tick, format!("{} jumped home", self.player_name(player)));
                if let Some(at) = self.players.get(&player).and_then(|p| p.ship.as_ref()).map(|s| s.at(tick).unwrap_or(s.last()).p) {
                    self.effects.push_back(Effect { tick, at, kind: EffectKind::Warped });
                }
            }
            Event::ShieldSpent { tick, player } => {
                self.say(tick, format!("{}'s shield broke", self.player_name(player)));
                if player == self.my_id {
                    self.alert(tick, "Your shield broke".to_string(), false);
                }
                if let Some(p) = self.players.get_mut(&player) {
                    p.shield = false;
                    if let Some(ship) = p.ship.as_ref() {
                        let at = ship.at(tick).unwrap_or(ship.last()).p;
                        self.effects.push_back(Effect { tick, at, kind: EffectKind::ShieldBroken });
                    }
                }
            }
            Event::BaseDestroyed { tick, owner, by } => {
                let whose = self.player_name(owner).to_string();
                let text = match by {
                    Some(by) if by == owner => format!("{whose} destroyed their own base"),
                    Some(by) => format!("{} destroyed {whose}'s base", self.player_name(by)),
                    None => format!("{whose}'s base was destroyed"),
                };
                self.say(tick, text);
                let me = self.my_id;
                match by {
                    _ if owner == me => self.alert(tick, if by == Some(me) { "You destroyed your own base".to_string() } else { "Your base is destroyed".to_string() }, false),
                    Some(by) if by == me => self.alert(tick, format!("You destroyed {whose}'s base"), true),
                    _ => {}
                }
                if let Some(b) = self.bases.iter_mut().find(|b| b.owner == owner) {
                    b.hp = 0;
                    let j = b.slot as usize;
                    if let Some(row) = self.eph.get(tick).or_else(|| self.eph.get(self.head.saturating_sub(1))).filter(|r| j < r.x.len()) {
                        let at = Particle { x: row.x[j], y: row.y[j], vx: 0.0, vy: 0.0 };
                        self.effects.push_back(Effect { tick, at, kind: EffectKind::BaseDestroyed });
                    }
                }
                if let Some(p) = by.filter(|by| *by != owner).and_then(|by| self.players.get_mut(&by)) {
                    p.captures += 1;
                }
            }
            Event::RoundOver { tick, next_round_tick, winner } => {
                self.next_round_tick = Some(next_round_tick);
                self.holds.clear();
                self.sieges.clear();
                self.winner = winner;
                let text = match winner {
                    Some(w) => format!("{} wins the round", self.player_name(w)),
                    None => "Round over".to_string(),
                };
                self.say(tick, text);
            }
            Event::Pieces { tick, pieces } => {
                for p in pieces {
                    self.pieces.insert(p.id, PieceRep { kind: p.kind, track: Track::new(tick, p.p) });
                }
            }
            Event::PieceGone { tick, id, by } => {
                if let Some(piece) = self.pieces.get_mut(&id) {
                    piece.track.dies_at = Some(tick);
                    if by.is_some() {
                        let at = *piece.track.at(tick).unwrap_or(piece.track.last());
                        self.effects.push_back(Effect { tick, at, kind: EffectKind::Pickup(piece.kind) });
                    }
                }
            }
            Event::Loadout { tick, player, loadout } => self.loadout(tick, player, loadout),
            Event::Hash { .. } => {} // handled by the session
        }
    }

    /// Authoritative ship state from the server. Returns true if ours was different.
    pub fn apply_ship_check(&mut self, tick: Tick, player: PlayerId, state: ShipState) -> bool {
        self.players
            .get_mut(&player)
            .and_then(|p| p.ship.as_mut())
            .is_some_and(|ship| ship.correct(tick, state))
    }

    /// Step every entity forward to `target` (bounded by the available ephemeris).
    pub fn advance_to(&mut self, target: Tick, stats: &mut Stats) {
        let Some(eph_end) = self.eph.end_tick() else { return };
        let target = target.min(eph_end);
        let (rules, eph, scratch) = (&self.rules, &self.eph, &mut self.scratch);
        for p in self.players.values_mut() {
            let Some(ship) = p.ship.as_mut() else { continue };
            let (timeline, kits, wells) = (&p.timeline, &p.kits, &self.wells);
            let rolled = ship.advance(target, |s, t| {
                let row = eph.get(t)?;
                let hit = step_ship(s, timeline.at(t), &wells_at(wells, t, rules), &row.view(), rules, scratch);
                // What the ship carries or has bought shows from the tick it changed.
                if let Some(kit) = kits.range(..=t + 1).next_back().map(|k| *k.1).filter(|k| *k != s.kit) {
                    s.refit(kit);
                }
                Some(hit)
            });
            if rolled {
                stats.rollbacks += 1;
            }
            if ship.dies_at.is_some_and(|d| d <= target) {
                p.ship = None;
            }
        }
        for s in self.shells.values_mut() {
            s.track.advance(target, |p, t| {
                let row = eph.get(t)?;
                Some(step_particle(p, (0.0, 0.0), rules.shell_radius, &row.view(), rules, scratch))
            });
        }
        self.shells.retain(|_, s| s.track.dies_at.is_none_or(|d| d > target));
        for piece in self.pieces.values_mut() {
            piece.track.advance(target, |p, t| {
                let row = eph.get(t)?;
                Some(step_particle(p, (0.0, 0.0), rules.shell_radius, &row.view(), rules, scratch))
            });
        }
        self.pieces.retain(|_, p| p.track.dies_at.is_none_or(|d| d > target));
        let gone = self.rules.well_ticks as Tick + HISTORY_TICKS as Tick;
        self.wells.retain(|w| w.0 + gone > target);
        if target > self.head {
            self.head = target;
        }
        while self.effects.front().is_some_and(|e| e.tick + 3 * self.rules.tick_hz as Tick <= self.head) {
            self.effects.pop_front();
        }
        self.eph.trim_before(self.head.saturating_sub(HISTORY_TICKS as Tick + 8));
    }

    pub fn ship_at(&self, player: PlayerId, tick_f: f64) -> Option<Particle> {
        let ship = self.players.get(&player)?.ship.as_ref()?;
        sample(ship, tick_f, |s| s.p, self.rules.dt)
    }

    pub fn shell_at(&self, id: ShellId, tick_f: f64) -> Option<Particle> {
        sample(&self.shells.get(&id)?.track, tick_f, |p| *p, self.rules.dt)
    }

    /// Row for a fractional tick plus the seconds elapsed into it.
    pub fn row_at(&self, tick_f: f64) -> Option<(Arc<EphRow>, f64)> {
        let t0 = tick_f.max(0.0).floor() as Tick;
        let row = self.eph.get(t0)?;
        Some((row, (tick_f - t0 as f64) * self.rules.dt))
    }

    /// Position and velocity of body `j`, `tau` seconds into the row's tick.
    pub fn body_at(row: &EphRow, j: usize, tau: f64) -> Particle {
        Particle {
            x: row.x[j] + tau * (row.vx[j] + 0.5 * row.ax[j] * tau),
            y: row.y[j] + tau * (row.vy[j] + 0.5 * row.ay[j] * tau),
            vx: row.vx[j] + row.ax[j] * tau,
            vy: row.vy[j] + row.ay[j] * tau,
        }
    }

    /// Muzzle state of a shell fired now from our ship.
    pub fn shell_muzzle(&self, angle: u16, speed: f64) -> Option<Particle> {
        let s = self.my_ship()?.last().p;
        let (dx, dy) = math::angle_to_dir(angle);
        let speed = speed.clamp(self.rules.shell_speed_min, self.shell_speed_max());
        let muzzle = self.rules.ship_radius + 2.0 * self.rules.shell_radius;
        Some(Particle { x: s.x + dx * muzzle, y: s.y + dy * muzzle, vx: s.vx + dx * speed, vy: s.vy + dy * speed })
    }
}

impl PlayerRep {
    pub fn score(&self, rules: &GameRules) -> i64 {
        rules.score(self.kills, self.deaths, self.captures)
    }
}

impl World {
    /// What a player carries, has banked and has bought, from `tick` on; their ship's engine
    /// and tank follow.
    fn loadout(&mut self, tick: Tick, player: PlayerId, new: Loadout) {
        let Some(p) = self.players.get_mut(&player) else { return };
        let emptied = !p.cargo.is_empty() && new.cargo.is_empty();
        let raised = Upgrade::SHIP.into_iter().chain(Upgrade::BASE).find(|u| new.upgrades.tier(*u) > p.upgrades.tier(*u));
        let news = if player != self.my_id {
            None
        } else if let Some(u) = raised {
            Some((format!("{} {}", capitalised(u.name()), "I".repeat(new.upgrades.tier(u) as usize)), true))
        } else if emptied {
            Some(if new.bank != p.bank { (format!("Banked {}", p.cargo.describe()), true) } else { (format!("Lost {}", p.cargo.describe()), false) })
        } else {
            None
        };
        (p.cargo, p.bank, p.upgrades, p.items, p.magazine, p.shield, p.quick) = (new.cargo, new.bank, new.upgrades, new.items, new.magazine, new.shield, new.quick);
        let kit = shop::kit(&self.rules, &new.upgrades, &new.cargo);
        p.kits.insert(tick, kit);
        // (Only the one in force at the oldest tick still kept, and those after it.)
        let oldest = self.head.saturating_sub(HISTORY_TICKS as Tick);
        while p.kits.range(..=oldest).count() > 1 {
            p.kits.pop_first();
        }
        if let Some(ship) = p.ship.as_mut() {
            ship.mark_dirty(tick.saturating_sub(1));
        }
        if let Some((text, good)) = news {
            self.alert(tick, text, good);
        }
    }

    pub fn piece_at(&self, id: PieceId, tick_f: f64) -> Option<Particle> {
        sample(&self.pieces.get(&id)?.track, tick_f, |p| *p, self.rules.dt)
    }

    /// The fastest our guns fire a shell.
    pub fn shell_speed_max(&self) -> f64 {
        self.me().map_or(self.rules.shell_speed_max, |m| shop::shell_speed_max(&self.rules, &m.upgrades))
    }
}

fn capitalised(word: &str) -> String {
    let mut c = word.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}
