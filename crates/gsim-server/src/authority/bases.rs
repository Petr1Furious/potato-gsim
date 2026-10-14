//! The bases mode. Every player owns a base: a body that never moves, pushes other bodies
//! away and pulls ships and shells in like a gas giant. Ships respawn around their own base
//! while it stands; enemy shells, rams and siege orbits wear it down. A player whose base is
//! gone is out once their ship is lost, and the last player in play wins the round.

use super::*;

/// What ships and shells feel: Jupiter.
const BASE_PULL: f64 = 1.898e27;
/// What other bodies feel: a push, enough to clear the neighbourhood of a base but (unlike a
/// whole Jupiter of it) not to blow the world apart over a round.
const BASE_MASS: f64 = -1.0e26;
const BASE_RADIUS: f64 = 6.99e7;
/// Ships appear on a circular orbit this many base radii out: outside the siege band of an
/// enemy, and a cheap burn away from leaving.
const SPAWN_RADII: f64 = 40.0;
const SHELL_DAMAGE: u32 = 120;
/// A ship crashing into an enemy base (and dying for it).
const RAM_DAMAGE: u32 = 600;
/// What a ship on a qualifying orbit around an enemy base takes off per second.
const SIEGE_PER_SECOND: u32 = 60;

pub(super) struct BaseState {
    pub owner: PlayerId,
    /// The owner's name: whoever comes back under it gets the base back.
    pub name: String,
    pub slot: u32,
    pub hp: u32,
}

impl Authority {
    pub(super) fn base_list(&self) -> Vec<Base> {
        self.bases.iter().map(|b| Base { owner: b.owner, slot: b.slot, hp: b.hp }).collect()
    }

    /// One base for everyone who plays (not those an operator set to watch), evenly on a ring
    /// around the centre. Called with the new world's bodies, before it is built.
    pub(super) fn lay_bases(&mut self, sc: &mut Scenario) {
        self.bases.clear();
        if self.mode != GameMode::Bases {
            return;
        }
        let owners: Vec<(PlayerId, String)> = self.players.values().filter(|p| !p.spectating).map(|p| (p.id, p.name.clone())).collect();
        let (ring, n) = (sc.spawn_r.0, owners.len().max(1) as f64);
        let phase = self.rng.angle();
        for (k, (owner, name)) in owners.into_iter().enumerate() {
            let a = phase + std::f64::consts::TAU * k as f64 / n;
            let slot = sc.bodies.len() as u32;
            sc.bodies.push(gsim_core::Body { x: ring * a.cos(), y: ring * a.sin(), vx: 0.0, vy: 0.0, mass: BASE_MASS, radius: BASE_RADIUS });
            sc.names.push((slot, format!("{name}'s base")));
            self.bases.push(BaseState { owner, name, slot, hp: BASE_HP });
        }
    }

    /// Fix the bases of the world just built in place.
    pub(super) fn anchor_bases(&mut self) {
        for b in &self.bases {
            self.massive.anchor(b.slot as usize, BASE_PULL);
        }
    }

    fn base_of(&self, player: PlayerId) -> Option<&BaseState> {
        self.bases.iter().find(|b| b.owner == player)
    }

    /// In the bases mode a ship respawns while its base stands (and, on an operator's word,
    /// even when it does not).
    pub(super) fn base_alive(&self, player: PlayerId) -> bool {
        self.base_of(player).is_some_and(|b| b.hp > 0)
    }

    /// A circular orbit around the player's base, if they have one.
    pub(super) fn base_spawn(&mut self, player: PlayerId) -> Option<ShipState> {
        let slot = self.base_of(player)?.slot as usize;
        let (m, a) = (&self.massive, self.rng.angle());
        let r = SPAWN_RADII * m.radius[slot];
        let v = (self.rules.g * m.pull[slot] / r).sqrt();
        let (c, s) = (a.cos(), a.sin());
        let p = Particle { x: m.x[slot] + r * c, y: m.y[slot] + r * s, vx: -v * s, vy: v * c };
        Some(ShipState::new(p, &self.rules))
    }

    /// Out of the round: watching until the next one.
    pub(super) fn eliminate(&mut self, player: PlayerId) {
        let tick = self.tick();
        let Some(p) = self.players.get_mut(&player).filter(|p| !p.spectating) else { return };
        (p.spectating, p.benched) = (true, true);
        (p.ship, p.respawn_tick, p.hold) = (None, None, None);
        self.event(Event::Gamemode { tick, player, spectating: true });
    }

    /// `by` hurts the base on `slot`, if it is somebody else's and still standing.
    pub(super) fn damage_base(&mut self, tick: Tick, slot: u32, by: PlayerId, amount: u32) {
        let Some(b) = self.bases.iter_mut().find(|b| b.slot == slot && b.hp > 0 && b.owner != by) else { return };
        b.hp = b.hp.saturating_sub(amount);
        self.bases_changed = true;
        if b.hp > 0 {
            return;
        }
        let owner = b.owner;
        if let Some(p) = self.players.get_mut(&by) {
            p.captures += 1;
        }
        self.event(Event::BaseDestroyed { tick, owner, by: Some(by) });
        // Somebody waiting for a ship gets none.
        if self.players.get(&owner).is_some_and(|p| p.ship.is_none()) {
            self.eliminate(owner);
        }
    }

    /// Once a tick in the bases mode: sieges wear bases down, the last player in play wins,
    /// and everyone hears how the bases stand.
    pub(super) fn tend_bases(&mut self, tick: Tick, playing: bool) {
        let rules = self.rules.clone();
        let mut sieges = Vec::new();
        if playing {
            for p in self.players.values() {
                let Some(ship) = p.ship else { continue };
                for b in self.bases.iter().filter(|b| b.hp > 0 && b.owner != p.id) {
                    let (m, j) = (&self.massive, b.slot as usize);
                    let body = Particle { x: m.x[j], y: m.y[j], vx: m.vx[j], vy: m.vy[j] };
                    if orbit_status(&ship.p, &body, m.pull[j], m.radius[j], &rules).ok {
                        sieges.push((p.id, b.owner));
                    }
                }
            }
        }
        // Spread evenly over the ticks of a second, in whole points.
        let hz = rules.tick_hz.max(1) as Tick;
        let per_tick = (SIEGE_PER_SECOND as Tick * (tick % hz + 1) / hz - SIEGE_PER_SECOND as Tick * (tick % hz) / hz) as u32;
        for (by, owner) in sieges.clone() {
            if let Some(slot) = self.base_of(owner).map(|b| b.slot) {
                self.damage_base(tick, slot, by, per_tick);
            }
        }
        if sieges != self.sieges {
            self.sieges = sieges;
            self.bases_changed = true;
        }
        // Who is left: everyone playing, and whoever dropped out with their base still standing
        // (they may come back for it).
        if playing && self.bases.len() >= 2 {
            let away = self.bases.iter().filter(|b| b.hp > 0 && !self.players.contains_key(&b.owner)).count();
            let mut left = self.players.values().filter(|p| !p.spectating).map(|p| p.id);
            if let (winner, None, 0) = (left.next(), left.next(), away) {
                self.end_round(tick, winner);
            }
        }
        if (self.bases_changed && tick.is_multiple_of(10)) || tick.is_multiple_of(hz) {
            self.bases_changed = false;
            let (bases, sieges) = (self.base_list(), self.sieges.clone());
            self.send(Target::All, ServerMsg::Bases { bases, sieges });
        }
    }

    /// A player who comes back under the name of a base's owner gets it back.
    pub(super) fn reclaim_base(&mut self, id: PlayerId, name: &str) -> Option<bool> {
        let b = self.bases.iter_mut().find(|b| b.name == name)?;
        b.owner = id;
        self.bases_changed = true;
        Some(b.hp > 0)
    }

    /// Switch modes: a new round starts in the new one (once somebody is here).
    pub fn set_mode(&mut self, mode: GameMode) {
        self.mode = mode;
        if !self.players.is_empty() {
            self.start_round();
        }
    }

    pub fn mode(&self) -> GameMode {
        self.mode
    }

    /// (owner, slot, hp) of every base.
    pub fn bases(&self) -> Vec<Base> {
        self.base_list()
    }

    pub fn spectating(&self, player: PlayerId) -> Option<bool> {
        self.players.get(&player).map(|p| p.spectating)
    }
}

/// Damage numbers, for tests.
pub const DAMAGE: (u32, u32, u32) = (SHELL_DAMAGE, RAM_DAMAGE, SIEGE_PER_SECOND);

impl Authority {
    /// Test hook: `by` takes `amount` off the base of `owner`.
    pub fn hurt_base(&mut self, owner: PlayerId, by: PlayerId, amount: u32) {
        let tick = self.tick();
        if let Some(slot) = self.base_of(owner).map(|b| b.slot) {
            self.damage_base(tick, slot, by, amount);
        }
    }
}
