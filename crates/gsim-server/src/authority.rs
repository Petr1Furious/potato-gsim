//! The authoritative world. It never rewinds: a command that arrives late is applied at the
//! current tick and the sender is told which tick that was.

use crate::rng::Rng;
use crate::scenario::{self, RandomOpts, Scenario};
use gsim_core::objective::orbit_status;
use gsim_core::particle::{step_particle, swept_min_dist2};
use gsim_core::ship::step_ship;
use gsim_core::{math, selftest, GameRules, InputTimeline, MassiveState, Particle, Scratch, ShipState, SystemFrame, Tick};
use gsim_proto::*;
use std::collections::{BTreeMap, VecDeque};

/// Transport-level connection handle.
pub type ConnId = u64;

pub enum Target {
    One(ConnId),
    /// Every joined player.
    All,
}

pub struct Outgoing {
    pub to: Vec<ConnId>,
    pub msg: ServerMsg,
}

struct Player {
    id: PlayerId,
    name: String,
    ship: Option<ShipState>,
    /// Belongs to the player, not the ship: held controls survive a respawn.
    timeline: InputTimeline,
    kills: u32,
    deaths: u32,
    captures: u32,
    /// Consecutive ticks on a qualifying orbit around the objective.
    hold: u32,
    respawn_tick: Option<Tick>,
    next_fire_tick: Tick,
    last_seq: u32,
    last_cmd_tick: Tick,
    fires: VecDeque<(Tick, u16, f32)>,
}

struct Shell {
    id: ShellId,
    owner: PlayerId,
    spawn_tick: Tick,
    p: Particle,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    /// Commands whose requested tick had already passed when they arrived.
    pub late_cmds: u64,
    pub cmds: u64,
    pub resyncs: u64,
    /// Why the objective moved: its body merged into another, vanished, or stopped qualifying.
    pub target_merged: u64,
    pub target_gone: u64,
    pub target_ineligible: u64,
    pub captures: u64,
}

pub struct Authority {
    pub rules: GameRules,
    pub massive: MassiveState,
    pub stats: Stats,
    preset: String,
    names: Vec<(u32, String)>,
    spawn_r: (f64, f64),
    players: BTreeMap<PlayerId, Player>,
    by_conn: BTreeMap<ConnId, PlayerId>,
    shells: Vec<Shell>,
    next_player_id: PlayerId,
    next_shell_id: ShellId,
    rng: Rng,
    out: Vec<Outgoing>,
    scratch: Scratch,
    /// Used to restart the same world when no generator is configured.
    initial: Scenario,
    generator: Option<(String, RandomOpts)>,
    round: u32,
    /// 0 = endless.
    round_ticks: Tick,
    intermission_ticks: Tick,
    round_end_tick: Option<Tick>,
    next_round_tick: Option<Tick>,
    target: Option<u32>,
    progress_sent: bool,
    objective_enabled: bool,
    /// A target should be chosen as soon as a ship is flying.
    target_pending: bool,
}

impl Authority {
    pub fn new(scenario: Scenario, rules: GameRules, seed: u64) -> Self {
        Self {
            initial: scenario.clone(),
            generator: None,
            round: 1,
            round_ticks: 0,
            intermission_ticks: 0,
            round_end_tick: None,
            next_round_tick: None,
            target: None,
            progress_sent: false,
            objective_enabled: false,
            target_pending: false,
            massive: MassiveState::from_bodies(&scenario.bodies),
            rules,
            stats: Stats::default(),
            preset: scenario.name,
            names: scenario.names,
            spawn_r: scenario.spawn_r,
            players: BTreeMap::new(),
            by_conn: BTreeMap::new(),
            shells: Vec::new(),
            next_player_id: 1,
            next_shell_id: 1,
            rng: Rng::new(seed ^ 0xA5A5_5A5A_1234_5678),
            out: Vec::new(),
            scratch: Scratch::default(),
        }
    }

    /// The tick the next [`Self::step`] will integrate.
    pub fn tick(&self) -> Tick {
        self.massive.tick
    }

    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    pub fn drain_out(&mut self) -> Vec<Outgoing> {
        std::mem::take(&mut self.out)
    }

    fn send(&mut self, target: Target, msg: ServerMsg) {
        let to = match target {
            Target::One(c) => vec![c],
            Target::All => self.by_conn.keys().copied().collect(),
        };
        if !to.is_empty() {
            self.out.push(Outgoing { to, msg });
        }
    }

    fn event(&mut self, e: Event) {
        self.send(Target::All, ServerMsg::Event(e));
    }

    pub fn disconnect(&mut self, conn: ConnId) {
        if let Some(id) = self.by_conn.remove(&conn) {
            self.players.remove(&id);
            self.event(Event::PlayerLeft { id });
        }
    }

    /// `tick_frac` is the fractional server tick at the moment the message was received.
    pub fn handle(&mut self, conn: ConnId, msg: ClientMsg, tick_frac: f64) {
        match msg {
            ClientMsg::Ping { client_time } => {
                self.send(Target::One(conn), ServerMsg::Pong { client_time, server_tick: tick_frac });
            }
            ClientMsg::Hello { protocol, golden, name } => self.hello(conn, protocol, golden, name),
            ClientMsg::Cmds(cmds) => self.commands(conn, cmds),
            ClientMsg::ResyncRequest => {
                if let Some(&id) = self.by_conn.get(&conn) {
                    self.stats.resyncs += 1;
                    let w = self.welcome(id);
                    self.send(Target::One(conn), ServerMsg::Welcome(Box::new(w)));
                }
            }
        }
    }

    fn hello(&mut self, conn: ConnId, protocol: u32, golden: u64, name: String) {
        if self.by_conn.contains_key(&conn) {
            return;
        }
        let name = clean_name(&name);
        let reject = if protocol != PROTOCOL_VERSION {
            Some(format!("protocol mismatch: server {PROTOCOL_VERSION}, client {protocol}"))
        } else if golden != selftest::GOLDEN {
            Some("simulation self-test mismatch: this build cannot stay in sync with the server".to_string())
        } else if name.is_empty() {
            Some("empty player name".to_string())
        } else if self.players.values().any(|p| p.name == name) {
            Some("that name is already in use".to_string())
        } else {
            None
        };
        if let Some(reason) = reject {
            self.send(Target::One(conn), ServerMsg::Reject { reason });
            return;
        }
        if self.players.is_empty() && self.round_ticks > 0 {
            // First player on an empty server: start a fresh round for them.
            self.start_round();
        }
        let id = self.next_player_id;
        self.next_player_id += 1;
        // Everyone already here learns about the newcomer; the newcomer learns everything
        // (itself included) from the snapshot.
        self.event(Event::PlayerJoined { id, name: name.clone() });
        self.players.insert(
            id,
            Player {
                id,
                name,
                ship: None,
                timeline: InputTimeline::new(),
                kills: 0,
                deaths: 0,
                captures: 0,
                hold: 0,
                respawn_tick: Some(self.tick() + 1),
                next_fire_tick: 0,
                last_seq: 0,
                last_cmd_tick: 0,
                fires: VecDeque::new(),
            },
        );
        self.by_conn.insert(conn, id);
        let w = self.welcome(id);
        self.send(Target::One(conn), ServerMsg::Welcome(Box::new(w)));
    }

    fn welcome(&self, your_id: PlayerId) -> Welcome {
        let now = self.tick();
        Welcome {
            your_id,
            preset: self.preset.clone(),
            round: self.round,
            round_end_tick: self.round_end_tick,
            next_round_tick: self.next_round_tick,
            target: self.target,
            rules: self.rules.clone(),
            massive: self.massive.snapshot(),
            names: self.names.clone(),
            players: self
                .players
                .values()
                .map(|p| {
                    let mut tl = p.timeline.clone();
                    tl.prune_before(now);
                    PlayerInfo {
                        id: p.id,
                        name: p.name.clone(),
                        kills: p.kills,
                        deaths: p.deaths,
                        captures: p.captures,
                        ship: p.ship,
                        inputs: tl.changes(),
                        respawn_tick: p.respawn_tick,
                        next_fire_tick: p.next_fire_tick,
                    }
                })
                .collect(),
            shells: self
                .shells
                .iter()
                .map(|s| ShellInfo { id: s.id, owner: s.owner, spawn_tick: s.spawn_tick, p: s.p })
                .collect(),
        }
    }

    fn commands(&mut self, conn: ConnId, cmds: Vec<Cmd>) {
        let Some(&id) = self.by_conn.get(&conn) else { return };
        let now = self.tick();
        let max_tick = now + self.rules.max_input_lead_ticks as Tick;
        let mut replies = Vec::new();
        let Some(p) = self.players.get_mut(&id) else { return };
        for cmd in cmds.into_iter().take(MAX_CMDS_PER_PACKET) {
            if cmd.seq <= p.last_seq {
                continue; // already processed, this is a redundant resend
            }
            if cmd.seq != p.last_seq + 1 {
                break; // gap: the missing command will arrive with a later packet
            }
            p.last_seq = cmd.seq;
            self.stats.cmds += 1;
            if cmd.tick < now {
                self.stats.late_cmds += 1;
            }
            // Never in the past, never before an earlier command, never absurdly far ahead.
            let tick = cmd.tick.max(now).max(p.last_cmd_tick).min(max_tick);
            p.last_cmd_tick = tick;
            match cmd.kind {
                CmdKind::Set(mut input) => {
                    input.thrust = input.thrust.min(100);
                    p.timeline.set(tick, input);
                    replies.push((Target::All, ServerMsg::Event(Event::Input { player: id, tick, input })));
                }
                CmdKind::Fire { angle, speed } => p.fires.push_back((tick, angle, speed)),
            }
            replies.push((Target::One(conn), ServerMsg::CmdAck { seq: cmd.seq, tick, accepted: true }));
        }
        for (target, msg) in replies {
            self.send(target, msg);
        }
    }

    /// Circular orbit around the barycentre, clear of every body, near the objective if any.
    fn spawn_state(&mut self) -> ShipState {
        let m = &self.massive;
        let (mut w, mut cx, mut cy, mut cvx, mut cvy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for j in 0..m.len() {
            let a = m.mass[j].abs();
            w += a;
            cx += m.x[j] * a;
            cy += m.y[j] * a;
            cvx += m.vx[j] * a;
            cvy += m.vy[j] * a;
        }
        if w > 0.0 {
            cx /= w;
            cy /= w;
            cvx /= w;
            cvy /= w;
        }
        // With an objective, spawn in a ring around it: near enough to be in the game at
        // once, far enough (several times the scoring band) that respawning is never a
        // shortcut to the target. Otherwise anywhere in the scenario's spawn annulus.
        let near = self.target.filter(|t| m.alive[*t as usize]).map(|t| {
            let j = t as usize;
            let lo = (0.12 * self.spawn_r.1).max(4.0 * self.rules.orbit_max_apo_radii * m.radius[j]);
            (m.x[j], m.y[j], lo, 2.0 * lo)
        });
        let mut best = Particle { x: cx + self.spawn_r.1, y: cy, vx: cvx, vy: cvy };
        for _ in 0..64 {
            let a = self.rng.angle();
            let (x, y) = match near {
                Some((tx, ty, lo, hi)) => {
                    let d = self.rng.range(lo, hi);
                    (tx + d * a.cos(), ty + d * a.sin())
                }
                None => {
                    let r = self.rng.range(self.spawn_r.0, self.spawn_r.1);
                    (cx + r * a.cos(), cy + r * a.sin())
                }
            };
            let (rx, ry) = (x - cx, y - cy);
            let r = (rx * rx + ry * ry).sqrt().max(1.0);
            let mut enclosed = 0.0;
            let mut clear = true;
            for j in 0..m.len() {
                if !m.alive[j] {
                    continue;
                }
                let (dcx, dcy) = (m.x[j] - cx, m.y[j] - cy);
                if dcx * dcx + dcy * dcy < r * r {
                    enclosed += m.mass[j];
                }
                let (dx, dy) = (m.x[j] - x, m.y[j] - y);
                let keep_out = 20.0 * m.radius[j] + 4.0 * self.rules.shell_blast_radius;
                if dx * dx + dy * dy < keep_out * keep_out {
                    clear = false;
                }
            }
            // Circular orbit about the barycentre through the chosen point.
            let v = (self.rules.g * enclosed.max(0.0) / r).sqrt();
            best = Particle { x, y, vx: cvx - v * ry / r, vy: cvy + v * rx / r };
            if clear {
                break;
            }
        }
        ShipState::new(best, &self.rules)
    }

    /// Integrate one tick.
    pub fn step(&mut self) {
        // An empty server does not start rounds: the next one begins when somebody joins.
        if self.next_round_tick.is_some_and(|n| self.tick() >= n) && !self.players.is_empty() {
            self.start_round();
        }
        let t = self.tick();
        let rules = self.rules.clone();

        // Respawns: the ship exists from the start of tick `t`.
        let due: Vec<PlayerId> =
            self.players.values().filter(|p| p.respawn_tick.is_some_and(|r| r <= t)).map(|p| p.id).collect();
        for id in due {
            let state = self.spawn_state();
            let p = self.players.get_mut(&id).unwrap();
            p.ship = Some(state);
            p.respawn_tick = None;
            self.event(Event::ShipSpawn { tick: t, player: id, state });
        }

        // Shells leave the muzzle from the ship's state at the start of tick `t`.
        let mut spawned = Vec::new();
        for p in self.players.values_mut() {
            while p.fires.front().is_some_and(|f| f.0 <= t) {
                let (_, angle, speed) = p.fires.pop_front().unwrap();
                let Some(ship) = p.ship else { continue };
                if t < p.next_fire_tick {
                    continue;
                }
                p.next_fire_tick = t + rules.shell_cooldown_ticks as Tick;
                let (dx, dy) = math::angle_to_dir(angle);
                let speed = if speed.is_finite() { speed as f64 } else { 0.0 };
                let speed = speed.clamp(rules.shell_speed_min, rules.shell_speed_max);
                let muzzle = rules.ship_radius + 2.0 * rules.shell_radius;
                let particle = Particle {
                    x: ship.p.x + dx * muzzle,
                    y: ship.p.y + dy * muzzle,
                    vx: ship.p.vx + dx * speed,
                    vy: ship.p.vy + dy * speed,
                };
                spawned.push(Shell { id: 0, owner: p.id, spawn_tick: t, p: particle });
            }
        }
        for mut s in spawned {
            s.id = self.next_shell_id;
            self.next_shell_id += 1;
            self.event(Event::ShellSpawn { tick: t, id: s.id, owner: s.owner, p: s.p });
            self.shells.push(s);
        }

        self.massive.ensure_acc(&rules);
        let view = self.massive.view();

        // (victim, killer, body)
        let mut deaths: Vec<(PlayerId, Option<PlayerId>, Option<u32>)> = Vec::new();
        // Ship segments over this tick, for the proximity fuses.
        let mut segs: Vec<(PlayerId, (f64, f64), (f64, f64))> = Vec::new();
        for p in self.players.values_mut() {
            let Some(ship) = p.ship.as_mut() else { continue };
            let before = (ship.p.x, ship.p.y);
            match step_ship(ship, p.timeline.at(t), &view, &rules, &mut self.scratch) {
                Some(body) => deaths.push((p.id, None, Some(body))),
                None => segs.push((p.id, before, (ship.p.x, ship.p.y))),
            }
        }

        let reach2 = (rules.shell_blast_radius + rules.ship_radius) * (rules.shell_blast_radius + rules.ship_radius);
        let mut gone: Vec<(ShellId, bool)> = Vec::new();
        for s in &mut self.shells {
            let before = (s.p.x, s.p.y);
            let hit = step_particle(&mut s.p, (0.0, 0.0), rules.shell_radius, &view, &rules, &mut self.scratch);
            let age = t + 1 - s.spawn_tick;
            if hit.is_some() {
                gone.push((s.id, false));
                continue;
            }
            if age >= rules.shell_arm_ticks as Tick {
                let after = (s.p.x, s.p.y);
                let mut exploded = false;
                for (victim, a0, a1) in &segs {
                    if swept_min_dist2(before, after, *a0, *a1) <= reach2 {
                        exploded = true;
                        if !deaths.iter().any(|d| d.0 == *victim) {
                            deaths.push((*victim, Some(s.owner), None));
                        }
                    }
                }
                if exploded {
                    gone.push((s.id, true));
                    continue;
                }
            }
            if age >= rules.shell_lifetime_ticks as Tick {
                gone.push((s.id, false));
            }
        }

        let merges = self.massive.step(&rules);
        let next = t + 1;

        for (id, exploded) in gone {
            self.shells.retain(|s| s.id != id);
            self.event(Event::ShellGone { tick: next, id, exploded });
        }
        let playing_now = self.next_round_tick.is_none();
        for (victim, killer, body) in deaths {
            let respawn_tick = next + rules.respawn_ticks as Tick;
            if let Some(p) = self.players.get_mut(&victim) {
                p.ship = None;
                p.deaths += playing_now as u32;
                p.hold = 0;
                p.respawn_tick = Some(respawn_tick);
            }
            if let Some(k) = killer.filter(|k| *k != victim && playing_now) {
                if let Some(p) = self.players.get_mut(&k) {
                    p.kills += 1;
                }
            }
            self.event(Event::ShipDied { tick: next, player: victim, killer, body, respawn_tick });
        }

        // --- objective and rounds ---
        let playing = self.next_round_tick.is_none();
        if self.target_pending && playing {
            self.pick_target(next, true);
        }
        // The objective follows its body into whatever absorbed it.
        let merged_into = self.target.and_then(|old| merges.iter().find(|e| e.absorbed.contains(&old))).and_then(|e| e.survivor);
        if let Some(survivor) = merged_into {
            self.target = Some(survivor);
            self.stats.target_merged += 1;
            self.event(Event::Objective { tick: next, target: Some(survivor) });
        }
        // Re-pick if the target is gone (annihilated or escaped), or (checked once a second) is
        // drifting out of play.
        let lost = self.target.is_some_and(|s| {
            let j = s as usize;
            !self.massive.alive[j]
                || (next % rules.tick_hz as Tick == 0 && {
                    let view = self.massive.kinematics();
                    let out = !self.target_eligible(&SystemFrame::of(&view), j, 0.5, false);
                    self.stats.target_ineligible += out as u64;
                    out
                })
        });
        if lost {
            self.stats.target_gone += self.target.is_some_and(|s| !self.massive.alive[s as usize]) as u64;
            self.pick_target(next, true);
        }
        if let (true, Some(slot)) = (playing, self.target) {
            let j = slot as usize;
            let m = &self.massive;
            let body = Particle { x: m.x[j], y: m.y[j], vx: m.vx[j], vy: m.vy[j] };
            let (mass, radius) = (m.mass[j], m.radius[j]);
            let mut winner = None;
            for p in self.players.values_mut() {
                let ok = p.ship.is_some_and(|s| orbit_status(&s.p, &body, mass, radius, &rules).ok);
                p.hold = if ok { p.hold + 1 } else { 0 };
                if p.hold >= rules.hold_ticks && winner.is_none() {
                    winner = Some(p.id);
                }
            }
            if let Some(id) = winner {
                if let Some(p) = self.players.get_mut(&id) {
                    p.captures += 1;
                }
                self.stats.captures += 1;
                self.event(Event::Captured { tick: next, player: id, target: slot });
                self.pick_target(next, true);
            }
            let any = self.players.values().any(|p| p.hold > 0);
            if next % 10 == 0 && (any || self.progress_sent) {
                let holds = self.players.values().filter(|p| p.hold > 0).map(|p| (p.id, p.hold)).collect();
                self.send(Target::All, ServerMsg::Progress { holds });
                self.progress_sent = any;
            }
        }
        if playing && self.round_end_tick.is_some_and(|e| next >= e) {
            let next_round_tick = next + self.intermission_ticks.max(1);
            self.next_round_tick = Some(next_round_tick);
            self.event(Event::RoundOver { tick: next, next_round_tick });
        }

        if next % rules.hash_interval_ticks.max(1) as Tick == 0 {
            let hash = self.massive.hash();
            self.event(Event::Hash { tick: next, hash });
        }
        let interval = rules.ship_check_interval_ticks.max(1) as Tick;
        let checks: Vec<(PlayerId, ShipState)> = self
            .players
            .values()
            .filter(|p| (next + p.id as Tick) % interval == 0)
            .filter_map(|p| p.ship.map(|s| (p.id, s)))
            .collect();
        for (player, state) in checks {
            self.send(Target::All, ServerMsg::ShipCheck { tick: next, player, state });
        }
        if next % 256 == 0 {
            for p in self.players.values_mut() {
                p.timeline.prune_before(next);
            }
        }
    }
}

/// Introspection and test hooks.
impl Authority {
    pub fn player_of(&self, conn: ConnId) -> Option<PlayerId> {
        self.by_conn.get(&conn).copied()
    }

    pub fn ship(&self, player: PlayerId) -> Option<ShipState> {
        self.players.get(&player).and_then(|p| p.ship)
    }

    pub fn captures(&self, player: PlayerId) -> Option<u32> {
        self.players.get(&player).map(|p| p.captures)
    }

    pub fn target(&self) -> Option<u32> {
        self.target
    }

    pub fn round(&self) -> u32 {
        self.round
    }

    pub fn score(&self, player: PlayerId) -> Option<(u32, u32)> {
        self.players.get(&player).map(|p| (p.kills, p.deaths))
    }

    pub fn shell_count(&self) -> usize {
        self.shells.len()
    }

    /// Place a player's ship (announced to everyone as a fresh spawn at the current tick).
    pub fn place_ship(&mut self, player: PlayerId, state: ShipState) {
        let tick = self.tick();
        if let Some(p) = self.players.get_mut(&player) {
            p.ship = Some(state);
            p.respawn_tick = None;
            self.event(Event::ShipSpawn { tick, player, state });
        }
    }
}

/// Rounds and the orbit objective.
impl Authority {
    /// Enable timed rounds. `generator` builds each new world (`None`: replay the first one).
    pub fn set_rounds(&mut self, round_ticks: Tick, intermission_ticks: Tick, generator: Option<(String, RandomOpts)>) {
        self.round_ticks = round_ticks;
        self.intermission_ticks = intermission_ticks;
        self.generator = generator;
        if self.players.is_empty() && round_ticks > 0 {
            // Nobody is here yet: round 1 starts when the first player joins.
            self.round = 0;
            self.round_end_tick = None;
        } else {
            self.round_end_tick = (round_ticks > 0).then(|| self.tick() + round_ticks);
        }
    }

    /// Turn the objective on (it starts off so that bare test worlds stay quiet).
    pub fn enable_objective(&mut self) {
        self.objective_enabled = true;
        let tick = self.tick();
        self.pick_target(tick, true);
    }

    /// Move the objective to a heavy body near the players; stars are skipped when anything
    /// else exists.
    fn pick_target(&mut self, tick: Tick, announce: bool) {
        for p in self.players.values_mut() {
            p.hold = 0;
        }
        // Where the players are: the next target should be within reach of them.
        let ships: Vec<Particle> = self.players.values().filter_map(|p| p.ship.map(|s| s.p)).collect();
        if ships.is_empty() {
            // Nobody is flying right now; choose once somebody is.
            if announce && self.target.is_some() {
                self.event(Event::Objective { tick, target: None });
            }
            self.target = None;
            self.target_pending = true;
            return;
        }
        let n = ships.len() as f64;
        let (cx, cy) = (ships.iter().map(|s| s.x).sum::<f64>() / n, ships.iter().map(|s| s.y).sum::<f64>() / n);
        // Only bodies that are part of the system: inside the core region and not on their way out.
        // (slot, mass, squared distance from the players)
        let mut cands: Vec<(u32, f64, f64)> = {
            let view = self.massive.kinematics();
            let frame = SystemFrame::of(&view);
            (0..view.x.len())
                .filter(|&j| self.target_eligible(&frame, j, 0.35, true))
                .map(|j| (j as u32, view.mass[j], (view.x[j] - cx) * (view.x[j] - cx) + (view.y[j] - cy) * (view.y[j] - cy)))
                .collect()
        };
        if cands.iter().any(|c| c.1 < 1.0e29) {
            cands.retain(|c| c.1 < 1.0e29);
        }
        if cands.len() > 1 {
            cands.retain(|c| Some(c.0) != self.target);
        }
        // The heavier half: their orbits are roomy enough to fly.
        cands.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let keep = (cands.len() / 2).max(4).min(cands.len());
        cands.truncate(keep);
        // Then one of the few closest to the players, so it is not always the very nearest.
        cands.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
        cands.truncate(3);
        let target = if cands.is_empty() { None } else { Some(cands[(self.rng.u64() % cands.len() as u64) as usize].0) };
        if announce && (target.is_some() || self.target.is_some()) {
            self.event(Event::Objective { tick, target });
        }
        self.target = target;
        self.target_pending = false;
    }

    /// Replace the world, reset scores and hand everyone a fresh snapshot.
    fn start_round(&mut self) {
        let tick = self.tick();
        let seed = self.rng.u64();
        let sc = match &self.generator {
            Some((preset, opts)) => scenario::build(preset, seed, opts).unwrap_or_else(|_| self.initial.clone()),
            None => self.initial.clone(),
        };
        self.massive = MassiveState::from_bodies(&sc.bodies);
        // Ticks never restart: clients keep their clock across rounds.
        self.massive.tick = tick;
        self.rules.escape_radius = if self.rules.escape_radius > 0.0 { sc.escape_radius() } else { 0.0 };
        self.names = sc.names;
        self.spawn_r = sc.spawn_r;
        self.shells.clear();
        self.round += 1;
        self.next_round_tick = None;
        self.round_end_tick = (self.round_ticks > 0).then(|| tick + self.round_ticks);
        self.target = None;
        self.target_pending = self.objective_enabled;
        for p in self.players.values_mut() {
            p.ship = None;
            p.kills = 0;
            p.deaths = 0;
            p.captures = 0;
            p.hold = 0;
            p.respawn_tick = Some(tick);
            p.fires.clear();
        }
        let ids: Vec<(ConnId, PlayerId)> = self.by_conn.iter().map(|(c, p)| (*c, *p)).collect();
        for (conn, id) in ids {
            let w = self.welcome(id);
            self.send(Target::One(conn), ServerMsg::Welcome(Box::new(w)));
        }
    }
}


impl Authority {
    /// A body worth orbiting: positive mass and within `frac` of the escape radius from the
    /// barycentre. With `settled`, also not currently moving outwards faster than escape speed.
    ///
    /// `settled` is only for choosing a new target. A body swinging past a heavy neighbour
    /// exceeds the escape speed for a moment without going anywhere, so an existing target
    /// is judged on distance alone; one that really leaves crosses the distance limit soon.
    fn target_eligible(&self, frame: &SystemFrame, j: usize, frac: f64, settled: bool) -> bool {
        let m = &self.massive;
        if !(m.alive[j] && m.mass[j] > 0.0) {
            return false;
        }
        let (r, escaping) = frame.escape_state(&m.kinematics(), j, self.rules.g);
        let limit = self.rules.escape_radius;
        !(settled && escaping) && (limit <= 0.0 || r <= frac * limit)
    }
}

fn clean_name(name: &str) -> String {
    name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect()
}

impl Authority {
    /// Connection currently playing under `name`, if any.
    pub fn holder_of(&self, name: &str) -> Option<ConnId> {
        let name = clean_name(name);
        let id = self.players.values().find(|p| p.name == name)?.id;
        self.by_conn.iter().find(|(_, p)| **p == id).map(|(c, _)| *c)
    }
}

impl Authority {
    /// Force the objective onto a body (tests).
    pub fn set_target(&mut self, slot: u32) {
        let tick = self.tick();
        self.objective_enabled = true;
        self.target_pending = false;
        self.target = Some(slot);
        self.event(Event::Objective { tick, target: Some(slot) });
    }
}
