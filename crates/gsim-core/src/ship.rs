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

/// What a ship's engine and tank can do: upgrades and the weight of its cargo change it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kit {
    /// Delta-v taken from the tank per tick at full thrust, in mm/s.
    pub burn: i64,
    /// Delta-v that gives the ship per tick at full thrust (less than `burn` when it is
    /// laden), in mm/s.
    pub push: i64,
    /// The most delta-v the tank holds, in mm/s.
    pub tank: i64,
}

impl Kit {
    /// A bare ship.
    pub fn of(rules: &GameRules) -> Self {
        Self { burn: rules.burn_per_tick_mmps, push: rules.burn_per_tick_mmps, tank: rules.fuel_max_mmps }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShipState {
    pub p: Particle,
    /// Remaining delta-v in mm/s.
    pub fuel: i64,
    /// Ticks since the engine last fired (drives regeneration).
    pub idle_ticks: u32,
    pub kit: Kit,
}

impl ShipState {
    /// A bare ship with a full tank.
    pub fn new(p: Particle, rules: &GameRules) -> Self {
        Self::with_kit(p, Kit::of(rules))
    }

    pub fn with_kit(p: Particle, kit: Kit) -> Self {
        Self { p, fuel: kit.tank, idle_ticks: 0, kit }
    }

    /// Another engine and tank from now on; a bigger tank comes with the extra delta-v.
    pub fn refit(&mut self, kit: Kit) {
        self.fuel += (kit.tank - self.kit.tank).max(0);
        self.kit = kit;
    }
}

/// A ship's stock of shells. Each shot takes one; once the guns have been quiet for a
/// while they come back one at a time, as delta-v does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Magazine {
    /// Shells left right after the last shot (or when it was filled).
    pub left: u32,
    /// The tick of that shot.
    pub since: Tick,
    /// The most it holds.
    pub max: u32,
    /// Ticks between two shots.
    pub cooldown: u32,
    /// Ticks for one shell to come back.
    pub regen: u32,
}

impl Magazine {
    /// The rules' magazine, full.
    pub fn full(tick: Tick, rules: &GameRules) -> Self {
        Self::new(tick, rules.shell_max, rules.shell_cooldown_ticks, rules.shell_regen_ticks)
    }

    /// A full magazine of `max` shells.
    pub fn new(tick: Tick, max: u32, cooldown: u32, regen: u32) -> Self {
        Self { left: max, since: tick, max, cooldown, regen: regen.max(1) }
    }

    /// Ticks between two shots.
    pub fn cooldown(&self) -> Tick {
        self.cooldown as Tick
    }

    /// Ticks for one shell to come back.
    fn regen_every(&self) -> Tick {
        self.regen.max(1) as Tick
    }

    /// Ticks towards shells coming back at `tick`: none until the delay is over.
    fn restoring(&self, tick: Tick, rules: &GameRules) -> Tick {
        tick.saturating_sub(self.since).saturating_sub(rules.shell_regen_delay_ticks as Tick)
    }

    /// Shells that can be fired at `tick`.
    pub fn at(&self, tick: Tick, rules: &GameRules) -> u32 {
        let back = self.restoring(tick, rules) / self.regen_every();
        (self.left as Tick + back).min(self.max as Tick) as u32
    }

    /// How far along the next shell is, 0 to 1 (0 while none is on its way).
    pub fn next(&self, tick: Tick, rules: &GameRules) -> f32 {
        let every = self.regen_every();
        if self.at(tick, rules) >= self.max {
            return 0.0;
        }
        (self.restoring(tick, rules) % every) as f32 / every as f32
    }

    /// Take a shell at `tick`. False, and nothing changes, if there is none.
    pub fn fire(&mut self, tick: Tick, rules: &GameRules) -> bool {
        let have = self.at(tick, rules);
        if have > 0 {
            *self = Self { left: have - 1, since: tick, ..*self };
        }
        have > 0
    }

    /// The same shells in another magazine (`new`, as it is when full): what the old one
    /// had at `tick`, and whatever more the new one holds.
    pub fn refit(&self, tick: Tick, new: Magazine, rules: &GameRules) -> Magazine {
        let have = self.at(tick, rules) + new.max.saturating_sub(self.max);
        Magazine { left: have.min(new.max), since: tick, ..new }
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
    let kit = s.kit;
    let want = kit.burn * (input.thrust.min(100) as i64) / 100;
    let used = want.min(s.fuel).max(0);
    let mut thrust = (0.0, 0.0);
    if used > 0 {
        s.fuel -= used;
        s.idle_ticks = 0;
        // A laden ship gets less out of the same burn.
        let gained = if kit.push == kit.burn { used } else { used * kit.push / kit.burn };
        let a = (gained as f64 * 1.0e-3) / rules.dt;
        let (dx, dy) = math::angle_to_dir(input.angle);
        thrust = (a * dx, a * dy);
    } else {
        s.idle_ticks = s.idle_ticks.saturating_add(1);
        if s.idle_ticks >= rules.regen_delay_ticks && s.fuel < kit.tank {
            s.fuel = (s.fuel + rules.regen_per_tick_mmps).min(kit.tank);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_come_back_after_a_quiet_spell() {
        let rules = GameRules::new(86400.0, 60);
        let secs = |s: f64| (s * 60.0) as Tick;
        let mut m = Magazine::full(0, &rules);
        assert_eq!(m.at(secs(100.0), &rules), 10);
        for shot in 0..10 {
            assert!(m.fire(secs(shot as f64), &rules));
        }
        // Empty since the shot at 9 s: four seconds of nothing, then one every two.
        assert!(!m.fire(secs(9.5), &rules));
        for (after, shells) in [(3.9, 0), (5.9, 0), (6.0, 1), (7.9, 1), (8.0, 2), (23.9, 9), (24.0, 10), (500.0, 10)] {
            assert_eq!(m.at(secs(9.0 + after), &rules), shells, "{after} s after the last shot");
        }
        assert_eq!(m.next(secs(9.0 + 3.0), &rules), 0.0);
        assert_eq!(m.next(secs(9.0 + 5.0), &rules), 0.5);
        assert_eq!(m.next(secs(9.0 + 60.0), &rules), 0.0);
        // A shot in between starts the wait over.
        assert!(m.fire(secs(9.0 + 7.0), &rules));
        assert_eq!(m.at(secs(9.0 + 12.9), &rules), 0);
        assert_eq!(m.at(secs(9.0 + 13.0), &rules), 1);
    }
}
