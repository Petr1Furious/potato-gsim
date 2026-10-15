//! The bases mode. Every player owns a base: a body that never moves, pushes other bodies
//! away and pulls ships and shells in like a small gas giant (within its reach). Ships respawn around their own base
//! while it stands; enemy shells, rams and siege orbits wear it down. A player whose base is
//! gone is out once their ship is lost, and the last player in play wins the round.

use super::*;

/// What ships and shells feel: an Earth, near the base (see [`REACH_RADII`]).
const BASE_PULL: f64 = 6.0e24;
/// What other bodies feel: a push that keeps the neighbourhood of a base clear, so that ships
/// circling it are not thrown about by passing bodies. (Three times as much hardly clears it
/// better and halves the bodies left near the centre within three minutes.)
const BASE_MASS: f64 = -3.0e25;
/// Earth's: small enough for a ship to circle close in.
const BASE_RADIUS: f64 = 6.4e6;
/// Ships feel a base's pull this many base radii out; beyond, it turns into the push bodies
/// feel (wholly at 1.41 times as far): a ship circling some planet far away is not dragged off
/// its orbit by bases the planet feels differently. The home zone ends there too;
/// the siege band and the spawn orbit are well inside.
const REACH_RADII: f64 = HOME_RADII;
/// Ships appear on a circular orbit this many base radii out (480 Mm, 0.9 km/s), inside the
/// home zone and high enough that full thrust is ten times what the base pulls with there: from
/// anywhere on the orbit, whichever way the ship is moving, a straight burn in any direction at
/// least 45° away from the base takes it clear.
const SPAWN_RADII: f64 = 75.0;
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
        let bases = self.mode == GameMode::Bases;
        self.rules.base_reach = if bases { REACH_RADII * BASE_RADIUS } else { 0.0 };
        if !bases {
            return;
        }
        let owners: Vec<(PlayerId, String)> = self.players.values().filter(|p| !p.spectating).map(|p| (p.id, p.name.clone())).collect();
        // Out where ships of the targets mode would spawn, as far apart as the world allows.
        let (ring, n) = (sc.spawn_r.1, owners.len().max(1) as f64);
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

    pub(super) fn base_of(&self, player: PlayerId) -> Option<&BaseState> {
        self.bases.iter().find(|b| b.owner == player)
    }

    /// In the bases mode a ship respawns while its base stands (and, on an operator's word,
    /// even when it does not).
    pub(super) fn base_alive(&self, player: PlayerId) -> bool {
        self.base_of(player).is_some_and(|b| b.hp > 0)
    }

    /// The player's ship is within their own standing base's home zone (see
    /// [`shop::zone_radii`]), where shells cost nothing and cargo is banked.
    pub(super) fn at_home(&self, player: PlayerId) -> bool {
        let Some(b) = self.base_of(player).filter(|b| b.hp > 0) else { return false };
        let Some(p) = self.players.get(&player) else { return false };
        let Some(s) = p.ship else { return false };
        let (m, j) = (&self.massive, b.slot as usize);
        let zone = shop::zone_radii(&p.upgrades) * m.radius[j];
        (s.p.x - m.x[j]).powi(2) + (s.p.y - m.y[j]).powi(2) <= zone * zone
    }

    /// A circular orbit around the player's base, if they have one.
    pub(super) fn base_spawn(&mut self, player: PlayerId) -> Option<Particle> {
        let slot = self.base_of(player)?.slot as usize;
        let (m, a) = (&self.massive, self.rng.angle());
        let r = SPAWN_RADII * m.radius[slot];
        let v = (self.rules.g * m.pull[slot] / r).sqrt();
        let (c, s) = (a.cos(), a.sin());
        let p = Particle { x: m.x[slot] + r * c, y: m.y[slot] + r * s, vx: -v * s, vy: v * c };
        Some(p)
    }

    /// Out of the round: watching until the next one.
    pub(super) fn eliminate(&mut self, player: PlayerId) {
        let tick = self.tick();
        let Some(p) = self.players.get_mut(&player).filter(|p| !p.spectating) else { return };
        (p.spectating, p.benched) = (true, true);
        (p.ship, p.respawn_tick, p.hold, p.shield) = (None, None, None, false);
        self.event(Event::Gamemode { tick, player, spectating: true });
    }

    /// `by` hurts the base on `slot` (their own too: shells and rams do not tell friend from
    /// foe), if it still stands, as much as its armour lets them.
    pub(super) fn damage_base(&mut self, tick: Tick, slot: u32, by: PlayerId, amount: u32) {
        let owner = self.bases.iter().find(|b| b.slot == slot).map(|b| b.owner);
        let amount = owner.and_then(|o| self.players.get(&o)).map_or(amount, |p| shop::armoured(amount, &p.upgrades));
        self.hurt(tick, slot, Some(by), amount);
    }

    /// `amount` off the base on `slot`, if it still stands (`by` nobody: sudden death).
    fn hurt(&mut self, tick: Tick, slot: u32, by: Option<PlayerId>, amount: u32) {
        let Some(b) = self.bases.iter_mut().find(|b| b.slot == slot && b.hp > 0) else { return };
        b.hp = b.hp.saturating_sub(amount);
        let (owner, left) = (b.owner, b.hp);
        self.bases_changed = true;
        if let Some(by) = by.filter(|by| !self.base_hits.contains(&(owner, *by))) {
            self.base_hits.push((owner, by));
        }
        if left > 0 {
            return;
        }
        if let Some(p) = by.filter(|by| *by != owner).and_then(|by| self.players.get_mut(&by)) {
            p.captures += 1;
        }
        self.event(Event::BaseDestroyed { tick, owner, by });
        // Somebody waiting for a ship gets none.
        if self.players.get(&owner).is_some_and(|p| p.ship.is_none()) {
            self.eliminate(owner);
        }
    }

    /// Once a tick in the bases mode: sieges wear bases down, the last player in play wins,
    /// and everyone hears how the bases stand.
    pub(super) fn tend_bases(&mut self, tick: Tick, merges: &[MergeEvent], playing: bool) {
        let rules = self.rules.clone();
        let holds = self.tend_mines(tick, merges, playing);
        self.tend_scrap(tick, playing);
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
        // Spread evenly over the ticks of a second, in whole points (as much a second as the
        // armour lets through).
        let hz = rules.tick_hz.max(1) as Tick;
        for (by, owner) in sieges.clone() {
            let rate = self.players.get(&owner).map_or(SIEGE_PER_SECOND, |p| shop::armoured(SIEGE_PER_SECOND, &p.upgrades)) as Tick;
            let per_tick = (rate * (tick % hz + 1) / hz - rate * (tick % hz) / hz) as u32;
            if let Some(slot) = self.base_of(owner).map(|b| b.slot) {
                self.hurt(tick, slot, Some(by), per_tick);
            }
        }
        // Sudden death: every standing base wears down.
        if playing && self.sudden_death.is_some_and(|t| tick >= t) {
            let rate = SUDDEN_DEATH_PER_SECOND as Tick;
            let per_tick = (rate * (tick % hz + 1) / hz - rate * (tick % hz) / hz) as u32;
            let standing: Vec<u32> = self.bases.iter().filter(|b| b.hp > 0).map(|b| b.slot).collect();
            for slot in standing {
                self.hurt(tick, slot, None, per_tick);
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
        if ((self.bases_changed || !holds.is_empty()) && tick.is_multiple_of(10)) || tick.is_multiple_of(hz) {
            self.bases_changed = false;
            let (bases, sieges, hits) = (self.base_list(), self.sieges.clone(), std::mem::take(&mut self.base_hits));
            self.send(Target::All, ServerMsg::Bases { bases, sieges, holds, hits });
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

impl Authority {
    /// A new round of bases rolls how much scrap there is, whether there is a mine more, and a
    /// twist; the rules start from the plain ones, bent by it.
    pub(super) fn roll_round(&mut self) {
        self.rules = self.plain_rules.clone();
        self.roll = Roll::default();
        if self.mode != GameMode::Bases {
            return;
        }
        let rng = &mut self.scrap_rng;
        let mut pick = |n: usize| (rng.u64() % n as u64) as usize;
        let rolled = Roll { scrap: [60, 100, 100, 160][pick(4)], mines: pick(2) as u32, twist: Twist::ALL[pick(Twist::ALL.len())] };
        let twist = self.forced_twist.unwrap_or(rolled.twist);
        self.roll = self.fixed_roll.unwrap_or(Roll { twist, ..rolled });
        let r = &mut self.rules;
        match self.roll.twist {
            Twist::DeepTanks => (r.fuel_max_mmps, r.regen_per_tick_mmps) = (2 * r.fuel_max_mmps, 2 * r.regen_per_tick_mmps),
            Twist::QuickGuns => (r.shell_cooldown_ticks, r.shell_regen_ticks) = (r.shell_cooldown_ticks / 2, r.shell_regen_ticks / 2),
            Twist::LongShots => (r.shell_speed_max, r.shell_lifetime_ticks) = (1.5 * r.shell_speed_max, 2 * r.shell_lifetime_ticks),
            _ => {}
        }
    }

    /// Test hook: every round from the next one on is rolled like this.
    pub fn fix_roll(&mut self, roll: Roll) {
        self.fixed_roll = Some(roll);
    }

    /// How this round differs from the others.
    pub fn roll(&self) -> Roll {
        self.roll
    }
}
