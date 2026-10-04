use crate::particle::{step_particle, Particle, Scratch};
use crate::{math, GameRules, MassiveView, Tick};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Quantised control state. Integers on the wire mean both sides derive identical thrust.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipInput {
    /// Heading, `0..65536` = full turn.
    pub angle: u16,
    /// Thrust percent, 0 = engine off.
    pub thrust: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShipState {
    pub p: Particle,
    /// Remaining delta-v in mm/s.
    pub fuel: i64,
    /// Ticks since the engine last fired (drives regeneration).
    pub idle_ticks: u32,
}

impl ShipState {
    pub fn new(p: Particle, rules: &GameRules) -> Self {
        Self { p, fuel: rules.fuel_max_mmps, idle_ticks: 0 }
    }
}

/// Advance a ship one tick under `input`. Fuel bookkeeping is pure integer arithmetic.
/// Returns the slot of the body the ship crashed into, if any.
pub fn step_ship(
    s: &mut ShipState,
    input: ShipInput,
    view: &MassiveView,
    rules: &GameRules,
    scratch: &mut Scratch,
) -> Option<u32> {
    let want = rules.burn_per_tick_mmps * (input.thrust.min(100) as i64) / 100;
    let used = want.min(s.fuel).max(0);
    let mut thrust = (0.0, 0.0);
    if used > 0 {
        s.fuel -= used;
        s.idle_ticks = 0;
        let a = (used as f64 * 1.0e-3) / rules.dt;
        let (dx, dy) = math::angle_to_dir(input.angle);
        thrust = (a * dx, a * dy);
    } else {
        s.idle_ticks = s.idle_ticks.saturating_add(1);
        if s.idle_ticks >= rules.regen_delay_ticks {
            s.fuel = (s.fuel + rules.regen_per_tick_mmps).min(rules.fuel_max_mmps);
        }
    }
    step_particle(&mut s.p, thrust, rules.ship_radius, view, rules, scratch)
}

/// "From tick T on, the input is I". The step that integrates tick T uses the latest change
/// at or before T.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputTimeline {
    changes: BTreeMap<Tick, ShipInput>,
}

impl InputTimeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_changes(changes: &[(Tick, ShipInput)]) -> Self {
        Self { changes: changes.iter().copied().collect() }
    }

    pub fn changes(&self) -> Vec<(Tick, ShipInput)> {
        self.changes.iter().map(|(t, i)| (*t, *i)).collect()
    }

    pub fn set(&mut self, tick: Tick, input: ShipInput) {
        self.changes.insert(tick, input);
    }

    pub fn remove(&mut self, tick: Tick) {
        self.changes.remove(&tick);
    }

    pub fn at(&self, tick: Tick) -> ShipInput {
        self.changes.range(..=tick).next_back().map(|(_, i)| *i).unwrap_or_default()
    }

    pub fn last_tick(&self) -> Option<Tick> {
        self.changes.keys().next_back().copied()
    }

    /// Drop changes that can no longer matter for ticks `>= tick`.
    pub fn prune_before(&mut self, tick: Tick) {
        let current = self.at(tick);
        self.changes = self.changes.split_off(&tick);
        self.changes.entry(tick).or_insert(current);
    }
}
