//! Client state machine: joins, keeps the clock in sync, schedules inputs slightly in the
//! future, and verifies that the local world matches the server's.

use crate::clock::TickClock;
use crate::world::World;
use gsim_core::{selftest, ShipInput, Tick};
use gsim_proto::*;
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub name: String,
    /// The key that proves who we are; the server ties our name to it.
    pub identity: Identity,
    /// Compute the ephemeris on a worker thread (GUI) or inline (tests, bots).
    pub threaded_eph: bool,
    /// How far ahead of the present the ephemeris is kept (ticks). Bounds prediction length.
    pub lookahead_ticks: u32,
    /// Inline mode: rows computed per update at most.
    pub inline_budget: usize,
}

impl SessionConfig {
    pub fn headless(name: &str) -> Self {
        Self { name: name.into(), identity: Identity::insecure_from_label(name), threaded_eph: false, lookahead_ticks: 8, inline_budget: 4000 }
    }
}

/// What the player wants right now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    pub angle: u16,
    pub thrust: u8,
    /// Fire a shell with this heading and muzzle speed (m/s).
    pub fire: Option<(u16, f32)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub hash_checks: u64,
    pub hash_mismatches: u64,
    pub resyncs: u64,
    /// Authoritative ship states that differed from ours.
    pub ship_corrections: u64,
    pub ship_checks: u64,
    /// Our commands the server applied at a different tick than requested.
    pub retimed_cmds: u64,
    pub cmds_sent: u64,
    /// Remote inputs that took effect in our past.
    pub late_remote_inputs: u64,
    pub rollbacks: u64,
}

pub struct Session {
    cfg: SessionConfig,
    pub world: Option<World>,
    clock: Option<TickClock>,
    out: Vec<ClientMsg>,
    pending: VecDeque<Cmd>,
    next_seq: u32,
    last_input: ShipInput,
    last_cmd_tick: Tick,
    last_facing_tick: Tick,
    fire_ready_tick: Tick,
    next_ping: f64,
    pings: u32,
    last_cmds_sent: f64,
    unchecked_hashes: VecDeque<(Tick, u64)>,
    awaiting_resync: bool,
    pub stats: Stats,
    /// Chat log, oldest first: messages, command output and game events.
    pub chat: VecDeque<ChatEntry>,
    /// Recent map markers from other players and ourselves.
    pub marks: VecDeque<Mark>,
    /// We may use operator commands.
    pub op: bool,
    pub rejected: Option<String>,
}

impl Session {
    pub fn new(cfg: SessionConfig) -> Self {
        let hello = ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            golden: selftest::compute(),
            name: clean_name(&cfg.name),
            key: cfg.identity.public(),
        };
        Self {
            cfg,
            world: None,
            clock: None,
            out: vec![hello],
            pending: VecDeque::new(),
            next_seq: 1,
            last_input: ShipInput::default(),
            last_cmd_tick: 0,
            last_facing_tick: 0,
            fire_ready_tick: 0,
            next_ping: 0.0,
            pings: 0,
            last_cmds_sent: f64::MIN,
            unchecked_hashes: VecDeque::new(),
            awaiting_resync: false,
            stats: Stats::default(),
            chat: VecDeque::new(),
            marks: VecDeque::new(),
            op: false,
            rejected: None,
        }
    }

    pub fn drain_out(&mut self) -> Vec<ClientMsg> {
        std::mem::take(&mut self.out)
    }

    pub fn joined(&self) -> bool {
        self.world.is_some()
    }

    /// Estimated fractional server tick: the instant we display.
    pub fn present(&self, now: f64) -> Option<f64> {
        self.clock.as_ref().map(|c| c.server_tick(now))
    }

    pub fn rtt(&self) -> Option<f64> {
        self.clock.as_ref().and_then(|c| c.rtt_last())
    }

    /// Ticks between our present and the tick our inputs are scheduled for.
    pub fn input_lead_ticks(&self) -> u32 {
        let Some(w) = &self.world else { return 0 };
        let hz = w.rules.tick_hz as f64;
        let one_way = self.clock.as_ref().and_then(|c| c.rtt_min()).unwrap_or(0.1) * 0.5;
        // Enough to reach the server before the tick is simulated, plus jitter headroom.
        let needed = (one_way * hz).ceil() as u32 + 2;
        needed.max(w.rules.input_delay_ticks).min(w.rules.max_input_lead_ticks / 2)
    }

    pub fn handle(&mut self, msg: ServerMsg, now: f64) {
        match msg {
            ServerMsg::Reject { reason } => self.rejected = Some(reason),
            ServerMsg::Challenge { nonce } => {
                let signature = self.cfg.identity.sign(&nonce, &clean_name(&self.cfg.name));
                self.out.push(ClientMsg::Auth { signature });
            }
            ServerMsg::Welcome(w) => self.on_welcome(&w, now),
            ServerMsg::Pong { client_time, server_tick } => {
                if let Some(c) = self.clock.as_mut() {
                    c.on_pong(client_time, server_tick, now);
                }
            }
            ServerMsg::CmdAck { seq, tick } => self.on_ack(seq, tick),
            ServerMsg::Event(Event::Hash { tick, hash }) => {
                self.unchecked_hashes.push_back((tick, hash));
                self.check_hashes();
            }
            ServerMsg::Event(e) => {
                if let Some(w) = self.world.as_mut() {
                    w.apply_event(e, &mut self.stats);
                    // What the world has to say about it goes into the chat log.
                    let news: Vec<String> = w.feed.drain(..).map(|f| f.1).collect();
                    for text in news {
                        self.log(now, ChatKind::System, None, text);
                    }
                }
            }
            ServerMsg::Chat(line) => self.log(now, line.kind, line.from, line.text),
            ServerMsg::Operator(op) => self.op = op,
            ServerMsg::Mark { player, x, y } => {
                let name = self.world.as_ref().map_or("?", |w| w.player_name(player)).to_string();
                self.marks.push_back(Mark { name, x, y, at: now });
                while self.marks.len() > 16 {
                    self.marks.pop_front();
                }
            }
            ServerMsg::Progress { objectives, holds } => {
                if let Some(w) = self.world.as_mut() {
                    // Which bodies are objectives comes with the events, in order; this only
                    // says how much time each has left.
                    for o in w.objectives.iter_mut() {
                        o.left = objectives.iter().find(|new| new.slot == o.slot).map_or(o.left, |new| new.left);
                    }
                    w.holds = holds.into_iter().filter(|h| w.is_target(h.1)).map(|(player, slot, held)| (player, (slot, held))).collect();
                }
            }
            ServerMsg::ShipCheck { tick, player, state } => {
                if let Some(w) = self.world.as_mut() {
                    self.stats.ship_checks += 1;
                    if w.apply_ship_check(tick, player, state) {
                        self.stats.ship_corrections += 1;
                    }
                }
            }
        }
    }

    fn on_welcome(&mut self, w: &Welcome, now: f64) {
        // A snapshot for the round we are already in means our copy had diverged; a new
        // round number is just the next world.
        let same_world = self.world.as_ref().is_some_and(|old| old.round == w.round);
        let resync = self.world.is_some();
        let mut world = World::from_welcome(w, self.cfg.threaded_eph);
        if resync {
            self.stats.resyncs += same_world as u64;
            self.awaiting_resync = false;
            self.unchecked_hashes.clear();
            // Commands the server has not acknowledged yet are still part of our plan.
            if let Some(me) = world.players.get_mut(&w.your_id) {
                for c in &self.pending {
                    if let CmdKind::Set(input) = c.kind {
                        me.timeline.set(c.tick, input);
                    }
                }
            }
        } else {
            self.clock = Some(TickClock::new(w.rules.tick_hz, w.massive.tick as f64, now));
            self.next_ping = now;
        }
        self.world = Some(world);
    }

    fn on_ack(&mut self, seq: u32, tick: Tick) {
        let Some(pos) = self.pending.iter().position(|c| c.seq == seq) else { return };
        let cmd = self.pending.remove(pos).unwrap();
        if tick == cmd.tick {
            return;
        }
        self.stats.retimed_cmds += 1;
        let CmdKind::Set(input) = cmd.kind else { return };
        let Some(w) = self.world.as_mut() else { return };
        let Some(me) = w.players.get_mut(&w.my_id) else { return };
        // The server applied it later than planned: move it and replay our ship from there.
        if me.timeline.at(cmd.tick) == input {
            me.timeline.remove(cmd.tick);
        }
        me.timeline.set(tick, input);
        if let Some(ship) = me.ship.as_mut() {
            ship.mark_dirty(cmd.tick.min(tick));
        }
    }

    fn check_hashes(&mut self) {
        let Some(w) = self.world.as_ref() else { return };
        let end = w.eph.end_tick().unwrap_or(0);
        while let Some(&(tick, hash)) = self.unchecked_hashes.front() {
            if tick >= end {
                break; // not computed yet
            }
            self.unchecked_hashes.pop_front();
            let Some(row) = w.eph.get(tick) else { continue }; // already trimmed
            self.stats.hash_checks += 1;
            if row.hash != hash && !self.awaiting_resync {
                self.stats.hash_mismatches += 1;
                self.awaiting_resync = true;
                self.out.push(ClientMsg::ResyncRequest);
            }
        }
    }

    /// Advance to `now` (seconds on any monotonic clock) with the player's current controls.
    pub fn update(&mut self, now: f64, controls: Controls) {
        let (Some(clock), Some(w)) = (self.clock.as_mut(), self.world.as_mut()) else { return };
        clock.update(now);
        if now >= self.next_ping {
            self.out.push(ClientMsg::Ping { client_time: now });
            self.pings += 1;
            // Converge quickly at first, then just track drift.
            self.next_ping = now + if self.pings < 12 { 0.05 } else { 0.5 };
        }

        let present = clock.server_tick(now).max(0.0).floor() as Tick;
        w.eph.request(present + 2 + self.cfg.lookahead_ticks as Tick, self.cfg.inline_budget);
        w.advance_to(present + 1, &mut self.stats);
        let synced = clock.synced();
        self.check_hashes();
        if !synced {
            return; // do not schedule inputs against an unsynchronised clock
        }

        let lead = self.input_lead_ticks();
        let Some(w) = self.world.as_mut() else { return };
        let hz = w.rules.tick_hz;
        let apply_tick = present + 1 + lead as Tick;
        let input = ShipInput { angle: controls.angle, thrust: controls.thrust.min(100) };
        // While coasting the heading is cosmetic: update it at a relaxed rate.
        let changed = input.thrust != self.last_input.thrust
            || (input.angle != self.last_input.angle
                && (input.thrust > 0 || apply_tick >= self.last_facing_tick + (hz / 10).max(1) as Tick));
        if changed && apply_tick > self.last_cmd_tick {
            if let Some(me) = w.players.get_mut(&w.my_id) {
                me.timeline.set(apply_tick, input);
                self.pending.push_back(Cmd { seq: self.next_seq, tick: apply_tick, kind: CmdKind::Set(input) });
                self.next_seq += 1;
                self.last_input = input;
                self.last_cmd_tick = apply_tick;
                self.last_facing_tick = apply_tick;
                self.stats.cmds_sent += 1;
            }
        }
        if let Some((angle, speed)) = controls.fire {
            let ready = w.me().map_or(Tick::MAX, |m| m.next_fire_tick).max(self.fire_ready_tick);
            let alive = w.my_ship().is_some();
            let loaded = w.me().is_some_and(|m| m.magazine.at(apply_tick, &w.rules) > 0);
            if alive && loaded && apply_tick >= ready && apply_tick >= self.last_cmd_tick {
                self.pending.push_back(Cmd { seq: self.next_seq, tick: apply_tick, kind: CmdKind::Fire { angle, speed } });
                self.next_seq += 1;
                self.last_cmd_tick = apply_tick;
                self.fire_ready_tick = apply_tick + w.rules.shell_cooldown_ticks as Tick;
                self.stats.cmds_sent += 1;
            }
        }
        // Resend everything unacknowledged once per tick: a lost packet costs nothing.
        if !self.pending.is_empty() && now - self.last_cmds_sent >= 1.0 / hz as f64 {
            self.last_cmds_sent = now;
            let cmds: Vec<Cmd> = self.pending.iter().take(MAX_CMDS_PER_PACKET).copied().collect();
            self.out.push(ClientMsg::Cmds(cmds));
        }
    }

    /// Tick at which the next shell can be fired.
    pub fn fire_ready_tick(&self) -> Tick {
        let server = self.world.as_ref().and_then(|w| w.me()).map_or(0, |m| m.next_fire_tick);
        server.max(self.fire_ready_tick)
    }
}

/// One line of the chat log.
#[derive(Clone, Debug)]
pub struct ChatEntry {
    /// Session time (seconds) it arrived at.
    pub at: f64,
    pub kind: ChatKind,
    pub from: Option<String>,
    pub text: String,
}

/// A spot somebody pointed at.
#[derive(Clone, Debug)]
pub struct Mark {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub at: f64,
}

impl Session {
    fn log(&mut self, at: f64, kind: ChatKind, from: Option<String>, text: String) {
        self.chat.push_back(ChatEntry { at, kind, from, text });
        while self.chat.len() > 200 {
            self.chat.pop_front();
        }
    }

    /// Say something, or run a `/command`.
    pub fn send_chat(&mut self, text: &str) {
        let text: String = text.trim().chars().take(MAX_CHAT_CHARS).collect();
        if !text.is_empty() {
            self.out.push(ClientMsg::Chat { text });
        }
    }

    /// Point at a spot on the map for everyone.
    pub fn send_mark(&mut self, x: f64, y: f64) {
        self.out.push(ClientMsg::Mark { x, y });
    }
}
