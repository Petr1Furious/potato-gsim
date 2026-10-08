//! Local replica of the game world. Massive bodies come from the ephemeris; every ship and
//! shell has its own history, so a late input or correction re-simulates only that entity.

use crate::eph::Eph;
use crate::session::Stats;
use gsim_core::particle::step_particle;
use gsim_core::ship::step_ship;
use gsim_core::{math, EphRow, GameRules, InputTimeline, Magazine, Particle, Scratch, ShipState, Tick};
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
}

/// Something to draw for a moment (drifts with the velocity it had).
#[derive(Clone, Copy, Debug)]
pub struct Effect {
    pub tick: Tick,
    pub at: Particle,
    pub kind: EffectKind,
}

pub struct World {
    pub rules: GameRules,
    pub my_id: PlayerId,
    pub preset: String,
    pub names: BTreeMap<u32, String>,
    pub eph: Eph,
    pub players: BTreeMap<PlayerId, PlayerRep>,
    pub shells: BTreeMap<ShellId, ShellRep>,
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
    scratch: Scratch,
}

impl World {
    pub fn from_welcome(w: &Welcome, threaded_eph: bool) -> Self {
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
                };
                (p.id, rep)
            })
            .collect();
        let shells = w
            .shells
            .iter()
            .map(|s| (s.id, ShellRep { owner: s.owner, spawn_tick: s.spawn_tick, track: Track::new(tick, s.p) }))
            .collect();
        Self {
            rules: w.rules.clone(),
            my_id: w.your_id,
            preset: w.preset.clone(),
            names: w.names.iter().cloned().collect(),
            eph: Eph::new(&w.massive, &w.rules, threaded_eph),
            players,
            shells,
            head: tick,
            effects: VecDeque::new(),
            feed: VecDeque::new(),
            scratch: Scratch::default(),
            round: w.round,
            round_end_tick: w.round_end_tick,
            next_round_tick: w.next_round_tick,
            objectives: w.objectives.clone(),
            holds: BTreeMap::new(),
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

    pub fn body_name(&self, slot: u32) -> String {
        self.names.get(&slot).cloned().unwrap_or_else(|| format!("B{slot}"))
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
            Event::ShipSpawn { tick, player, state } => {
                if let Some(p) = self.players.get_mut(&player) {
                    p.ship = Some(Track::new(tick, state));
                    p.respawn_tick = None;
                    // A new ship comes fully armed.
                    p.magazine = Magazine::full(tick, &self.rules);
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
            Event::ShellSpawn { tick, id, owner, p } => {
                self.shells.insert(id, ShellRep { owner, spawn_tick: tick, track: Track::new(tick, p) });
                if let Some(pl) = self.players.get_mut(&owner) {
                    pl.next_fire_tick = tick + self.rules.shell_cooldown_ticks as Tick;
                    pl.magazine.fire(tick, &self.rules);
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
                let counts = self.next_round_tick.is_none();
                if let Some(k) = killer.filter(|k| *k != player && counts) {
                    if let Some(p) = self.players.get_mut(&k) {
                        p.kills += 1;
                    }
                }
                if let Some(p) = self.players.get_mut(&player) {
                    p.deaths += counts as u32;
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
            Event::RoundOver { tick, next_round_tick } => {
                self.next_round_tick = Some(next_round_tick);
                self.holds.clear();
                self.say(tick, "Round over".to_string());
            }
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
            let timeline = &p.timeline;
            let rolled = ship.advance(target, |s, t| {
                let row = eph.get(t)?;
                Some(step_ship(s, timeline.at(t), &row.view(), rules, scratch))
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
        self.shells.retain(|_, s| !s.track.dies_at.is_some_and(|d| d <= target));
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
        let speed = speed.clamp(self.rules.shell_speed_min, self.rules.shell_speed_max);
        let muzzle = self.rules.ship_radius + 2.0 * self.rules.shell_radius;
        Some(Particle { x: s.x + dx * muzzle, y: s.y + dy * muzzle, vx: s.vx + dx * speed, vy: s.vy + dy * speed })
    }
}

impl PlayerRep {
    pub fn score(&self, rules: &GameRules) -> i64 {
        rules.score(self.kills, self.deaths, self.captures)
    }
}
