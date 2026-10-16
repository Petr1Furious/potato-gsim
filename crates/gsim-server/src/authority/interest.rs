//! Bodies of interest (targets, and mines in the bases mode) that stop being worth flying
//! to: one that drifts far from the center of the world or races away from it, and one swung
//! around by a much heavier neighbor, which leaves no room for an orbit around it.

use super::*;

/// An orbit around a body needs room: once the body's Hill sphere (where its own pull beats
/// the tides of a heavier neighbor) is smaller than this many of its radii, no qualifying
/// orbit around it lasts.
const HILL_RADII: f64 = 20.0;
/// A neighbor this many times as heavy swings a body around; one between `PAIR_MASS` and that
/// makes a pair with it, around either of which orbits have room only within `PAIR_ROOM` of the
/// gap between them.
const SWUNG_MASS: f64 = 2.0;
const PAIR_MASS: f64 = 0.3;
const PAIR_ROOM: f64 = 0.4;
/// Within 30° of straight at a body.
const AIM_COS: f64 = 0.866;
/// A body leaving the center faster than this share of a ship's whole delta-v is not worth
/// chasing.
const FAST_SHARE: f64 = 0.25;
/// When a body has to be replaced by one like it: how many seconds of play a difference in
/// velocity weighs as much as the distance it would open up.
const ALIKE_SECONDS: f64 = 10.0;

/// What is wrong with a body of interest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Trouble {
    /// Far from the center, or racing away from it, and nobody on the way to it.
    Far,
    /// Swung around by this much heavier body.
    Swung(u32),
    /// In a tight pair with a body of much the same mass: no orbit around either lasts.
    Paired,
}

impl Authority {
    /// How far from the center a body of interest may be: targets up to half again as far as
    /// the world reached when it was made, mines (which start near the center) within half
    /// of that.
    pub(super) fn interest_limit(&self, mine: bool) -> f64 {
        if mine { 0.5 * self.world_r } else { 1.5 * self.world_r }
    }

    pub(super) fn distance_from_center(&self, frame: &SystemFrame, j: usize) -> f64 {
        let m = &self.massive;
        ((m.x[j] - frame.x).powi(2) + (m.y[j] - frame.y).powi(2)).sqrt()
    }

    /// Whether the body on `slot` should stop being of interest, and why.
    pub(super) fn trouble(&self, frame: &SystemFrame, slot: u32, mine: bool) -> Option<Trouble> {
        let j = slot as usize;
        if let Some(heavy) = self.swung_by(j) {
            return Some(Trouble::Swung(heavy));
        }
        if self.paired(j) {
            return Some(Trouble::Paired);
        }
        // A body somebody is on the way to stays, or they would arrive at nothing.
        let away = self.distance_from_center(frame, j) > self.interest_limit(mine) || self.racing_away(frame, j);
        (away && !self.sought(j)).then_some(Trouble::Far)
    }

    /// Leaving the center too fast to be worth chasing.
    fn racing_away(&self, frame: &SystemFrame, j: usize) -> bool {
        let m = &self.massive;
        let (dx, dy) = (m.x[j] - frame.x, m.y[j] - frame.y);
        let (vx, vy) = (m.vx[j] - frame.vx, m.vy[j] - frame.vy);
        let outwards = (dx * vx + dy * vy) / (dx * dx + dy * dy).sqrt().max(1.0);
        outwards > FAST_SHARE * self.rules.fuel_max_mmps as f64 * 1.0e-3
    }

    /// Where a body of interest goes when the one swinging it around cannot take it over: the
    /// fit body nearest to it in place and in motion together, so that whoever was on the way
    /// still is, more or less.
    pub(super) fn most_alike(&self, j: usize, avoid: &[u32], mine: bool) -> Option<u32> {
        let frame = SystemFrame::of(&self.massive.kinematics());
        let m = &self.massive;
        let weight = ALIKE_SECONDS * self.rules.time_scale();
        let unlike = |k: usize| {
            let d = ((m.x[k] - m.x[j]).powi(2) + (m.y[k] - m.y[j]).powi(2)).sqrt();
            let dv = ((m.vx[k] - m.vx[j]).powi(2) + (m.vy[k] - m.vy[j]).powi(2)).sqrt();
            d + weight * dv
        };
        (0..m.len())
            .filter(|&k| k != j && !avoid.contains(&(k as u32)) && self.fit_for_interest(&frame, k, mine))
            .min_by(|a, b| unlike(*a).total_cmp(&unlike(*b)).then(a.cmp(b)))
            .map(|k| k as u32)
    }

    /// The much heavier body (twice as heavy or more) that leaves `j` no room for an orbit (the
    /// tightest squeeze, if several do). Bases are left out: ships feel them only up close.
    pub(super) fn swung_by(&self, j: usize) -> Option<u32> {
        let m = &self.massive;
        if !(m.alive[j] && m.mass[j] > 0.0) {
            return None;
        }
        let mut best: Option<(u32, f64)> = None;
        for k in (0..m.len()).filter(|&k| k != j && m.alive[k] && !m.fixed[k] && m.mass[k] > SWUNG_MASS * m.mass[j]) {
            let d = ((m.x[k] - m.x[j]).powi(2) + (m.y[k] - m.y[j]).powi(2)).sqrt();
            let hill = d * (m.mass[j] / (3.0 * m.mass[k])).cbrt();
            if hill < HILL_RADII * m.radius[j] && best.is_none_or(|b| hill < b.1) {
                best = Some((k as u32, hill));
            }
        }
        best.map(|b| b.0)
    }

    /// `j` is in a tight pair with a body of much the same mass (from 30 % of its own to twice it):
    /// the room for an orbit around either is then only a fraction of the gap between them, and
    /// no qualifying orbit lasts once that is too small.
    pub(super) fn paired(&self, j: usize) -> bool {
        let m = &self.massive;
        if !(m.alive[j] && m.mass[j] > 0.0) {
            return false;
        }
        (0..m.len()).filter(|&k| k != j && m.alive[k] && !m.fixed[k]).any(|k| {
            let ratio = m.mass[k] / m.mass[j];
            let d = ((m.x[k] - m.x[j]).powi(2) + (m.y[k] - m.y[j]).powi(2)).sqrt();
            (PAIR_MASS..=SWUNG_MASS).contains(&ratio) && PAIR_ROOM * d < HILL_RADII * m.radius[j]
        })
    }

    /// Somebody is on the way to `j`: flying straight at it, thrusting towards it, or already
    /// circling it.
    fn sought(&self, j: usize) -> bool {
        let (t, m) = (self.tick(), &self.massive);
        self.players.values().any(|p| {
            let Some(s) = p.ship else { return false };
            let (dx, dy) = (m.x[j] - s.p.x, m.y[j] - s.p.y);
            let d = (dx * dx + dy * dy).sqrt().max(1.0);
            let (vx, vy) = (s.p.vx - m.vx[j], s.p.vy - m.vy[j]);
            let v = (vx * vx + vy * vy).sqrt();
            let flying = v > 1.0 && dx * vx + dy * vy > AIM_COS * d * v;
            let input = p.timeline.at(t);
            let (hx, hy) = math::angle_to_dir(input.angle);
            let thrusting = input.thrust > 0 && s.fuel > 0 && dx * hx + dy * hy > AIM_COS * d;
            flying || thrusting || p.hold.is_some_and(|h| h.0 == j as u32)
        })
    }

    /// Fit to become a body of interest: there, positive, no base, near enough to the center
    /// (with room to drift), not racing away and not swung around by anything; and no star,
    /// for a mine.
    pub(super) fn fit_for_interest(&self, frame: &SystemFrame, j: usize, mine: bool) -> bool {
        let m = &self.massive;
        m.alive[j] && !m.fixed[j] && m.mass[j] > 0.0 && !(mine && m.mass[j] >= 1.0e29) && self.distance_from_center(frame, j) < 0.7 * self.interest_limit(mine) && !self.racing_away(frame, j) && self.swung_by(j).is_none() && !self.paired(j)
    }

    /// Move a target or a mine somewhere else at once (an operator's call).
    pub(super) fn reelect(&mut self, slot: u32) -> Result<u32, String> {
        let tick = self.tick();
        if let Some(k) = self.objectives.iter().position(|o| o.slot == slot) {
            let new = self.choose_target(&[slot]).filter(|s| *s != slot).ok_or("there is no other body to choose")?;
            self.objectives[k] = Objective { slot: new, left: self.rules.target_ticks };
            self.forget_holds(slot);
            let objectives = self.objectives.clone();
            self.event(Event::Objectives { tick, objectives });
            return Ok(new);
        }
        if let Some(k) = self.mines.iter().position(|s| s.slot == slot) {
            let new = self.choose_mine(&[slot]).ok_or("there is no other body to choose")?;
            self.mines[k].slot = new;
            self.forget_holds(slot);
            let mines = self.mines.clone();
            self.event(Event::Mines { tick, mines });
            return Ok(new);
        }
        Err("that body is neither a target nor a mine".into())
    }

    pub(super) fn forget_holds(&mut self, slot: u32) {
        for p in self.players.values_mut() {
            p.hold = p.hold.filter(|h| h.0 != slot);
        }
    }
}
