//! Gravitating bodies. Their evolution depends on nothing but their own previous state,
//! so it is a pure function of (initial state, tick) and never needs to be rolled back.

use crate::{math, GameRules, Tick};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::Xxh3;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub mass: f64,
    pub radius: f64,
}

/// Bodies that overlapped at the end of a tick. `survivor == None` means mutual annihilation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MergeEvent {
    pub survivor: Option<u32>,
    pub absorbed: Vec<u32>,
}

/// Everything needed to restart the massive tier at `tick`. Accelerations are not included:
/// they are a pure function of positions and masses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MassiveSnapshot {
    pub tick: Tick,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub vx: Vec<f64>,
    pub vy: Vec<f64>,
    pub mass: Vec<f64>,
    pub radius: Vec<f64>,
    pub alive: Vec<bool>,
}

/// Borrowed start-of-tick state, enough to integrate test particles across that tick.
#[derive(Clone, Copy)]
pub struct MassiveView<'a> {
    pub x: &'a [f64],
    pub y: &'a [f64],
    pub vx: &'a [f64],
    pub vy: &'a [f64],
    pub ax: &'a [f64],
    pub ay: &'a [f64],
    pub mass: &'a [f64],
    pub radius: &'a [f64],
    pub alive: &'a [bool],
}

/// Slots are stable for the lifetime of a match: merged bodies are tombstoned
/// (`alive = false`, mass 0), never compacted, so a slot index is a permanent id.
#[derive(Clone, Debug)]
pub struct MassiveState {
    pub tick: Tick,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub vx: Vec<f64>,
    pub vy: Vec<f64>,
    pub ax: Vec<f64>,
    pub ay: Vec<f64>,
    pub mass: Vec<f64>,
    pub radius: Vec<f64>,
    pub alive: Vec<bool>,
    acc_valid: bool,
    bx: Vec<f64>,
    by: Vec<f64>,
    hit: Vec<bool>,
}

const PARALLEL_MIN_BODIES: usize = 192;

/// Acceleration on every live body plus a per-body "overlaps something" flag.
/// Each output element is produced by one serial loop over `j`, so the result does not
/// depend on how rayon splits the work.
#[allow(clippy::too_many_arguments)]
fn accelerations(
    x: &[f64],
    y: &[f64],
    mass: &[f64],
    radius: &[f64],
    alive: &[bool],
    g: f64,
    eps2: f64,
    out_ax: &mut [f64],
    out_ay: &mut [f64],
    hit: &mut [bool],
) {
    let n = x.len();
    let one = |i: usize, ax: &mut f64, ay: &mut f64, h: &mut bool| {
        *h = false;
        if !alive[i] {
            *ax = 0.0;
            *ay = 0.0;
            return;
        }
        let (xi, yi, ri) = (x[i], y[i], radius[i]);
        let (mut sx, mut sy) = (0.0, 0.0);
        let mut any_hit = false;
        // Dead slots carry mass 0 and i == j gives dx = dy = 0, so neither needs a branch
        // in the force sum (softening keeps the denominator positive).
        for j in 0..n {
            let dx = x[j] - xi;
            let dy = y[j] - yi;
            let d2raw = dx * dx + dy * dy;
            let rr = ri + radius[j];
            if d2raw <= rr * rr && j != i && alive[j] {
                any_hit = true;
            }
            let d2 = d2raw + eps2;
            let inv = 1.0 / d2.sqrt();
            let f = g * mass[j] * inv * inv * inv;
            sx += dx * f;
            sy += dy * f;
        }
        *ax = sx;
        *ay = sy;
        *h = any_hit;
    };
    if n >= PARALLEL_MIN_BODIES {
        out_ax
            .par_iter_mut()
            .zip(out_ay.par_iter_mut())
            .zip(hit.par_iter_mut())
            .enumerate()
            .with_min_len(16)
            .for_each(|(i, ((ax, ay), h))| one(i, ax, ay, h));
    } else {
        for i in 0..n {
            let (mut ax, mut ay, mut h) = (0.0, 0.0, false);
            one(i, &mut ax, &mut ay, &mut h);
            out_ax[i] = ax;
            out_ay[i] = ay;
            hit[i] = h;
        }
    }
}

impl MassiveState {
    pub fn from_bodies(bodies: &[Body]) -> Self {
        let n = bodies.len();
        let col = |f: fn(&Body) -> f64| bodies.iter().map(f).collect::<Vec<f64>>();
        Self {
            tick: 0,
            x: col(|b| b.x),
            y: col(|b| b.y),
            vx: col(|b| b.vx),
            vy: col(|b| b.vy),
            ax: vec![0.0; n],
            ay: vec![0.0; n],
            mass: col(|b| b.mass),
            radius: col(|b| b.radius),
            alive: vec![true; n],
            acc_valid: false,
            bx: vec![0.0; n],
            by: vec![0.0; n],
            hit: vec![false; n],
        }
    }

    pub fn from_snapshot(s: &MassiveSnapshot) -> Self {
        let n = s.x.len();
        Self {
            tick: s.tick,
            x: s.x.clone(),
            y: s.y.clone(),
            vx: s.vx.clone(),
            vy: s.vy.clone(),
            ax: vec![0.0; n],
            ay: vec![0.0; n],
            mass: s.mass.clone(),
            radius: s.radius.clone(),
            alive: s.alive.clone(),
            acc_valid: false,
            bx: vec![0.0; n],
            by: vec![0.0; n],
            hit: vec![false; n],
        }
    }

    pub fn snapshot(&self) -> MassiveSnapshot {
        MassiveSnapshot {
            tick: self.tick,
            x: self.x.clone(),
            y: self.y.clone(),
            vx: self.vx.clone(),
            vy: self.vy.clone(),
            mass: self.mass.clone(),
            radius: self.radius.clone(),
            alive: self.alive.clone(),
        }
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn alive_count(&self) -> usize {
        self.alive.iter().filter(|a| **a).count()
    }

    /// Make `ax/ay` valid for the current positions (needed before [`Self::view`]).
    pub fn ensure_acc(&mut self, rules: &GameRules) {
        if self.acc_valid {
            return;
        }
        let eps2 = rules.softening * rules.softening;
        accelerations(
            &self.x, &self.y, &self.mass, &self.radius, &self.alive, rules.g, eps2,
            &mut self.ax, &mut self.ay, &mut self.hit,
        );
        self.acc_valid = true;
    }

    /// Start-of-tick view. Call [`Self::ensure_acc`] first.
    pub fn view(&self) -> MassiveView<'_> {
        debug_assert!(self.acc_valid);
        MassiveView {
            x: &self.x,
            y: &self.y,
            vx: &self.vx,
            vy: &self.vy,
            ax: &self.ax,
            ay: &self.ay,
            mass: &self.mass,
            radius: &self.radius,
            alive: &self.alive,
        }
    }

    /// Advance one tick (velocity Verlet), then merge overlapping bodies.
    pub fn step(&mut self, rules: &GameRules) -> Vec<MergeEvent> {
        self.ensure_acc(rules);
        let dt = rules.dt;
        let half_dt = 0.5 * dt;
        let half_dt2 = 0.5 * dt * dt;
        let n = self.len();
        for i in 0..n {
            self.x[i] += self.vx[i] * dt + self.ax[i] * half_dt2;
            self.y[i] += self.vy[i] * dt + self.ay[i] * half_dt2;
        }
        let eps2 = rules.softening * rules.softening;
        accelerations(
            &self.x, &self.y, &self.mass, &self.radius, &self.alive, rules.g, eps2,
            &mut self.bx, &mut self.by, &mut self.hit,
        );
        for i in 0..n {
            self.vx[i] += (self.ax[i] + self.bx[i]) * half_dt;
            self.vy[i] += (self.ay[i] + self.by[i]) * half_dt;
        }
        // The acceleration at the new positions is next tick's starting acceleration.
        std::mem::swap(&mut self.ax, &mut self.bx);
        std::mem::swap(&mut self.ay, &mut self.by);

        let mut events = Vec::new();
        if self.hit.iter().any(|h| *h) {
            self.merge_overlaps(rules, &mut events);
        }
        for i in 0..n {
            if self.alive[i]
                && !(self.x[i].is_finite()
                    && self.y[i].is_finite()
                    && self.vx[i].is_finite()
                    && self.vy[i].is_finite())
            {
                self.kill(i);
                events.push(MergeEvent { survivor: None, absorbed: vec![i as u32] });
            }
        }
        if rules.escape_radius > 0.0 && (self.tick + 1) % rules.escape_check_ticks.max(1) as Tick == 0 {
            self.remove_escapers(rules, &mut events);
        }
        if !events.is_empty() {
            self.acc_valid = false;
        }
        self.tick += 1;
        events
    }

    fn kill(&mut self, i: usize) {
        self.alive[i] = false;
        self.mass[i] = 0.0;
        self.radius[i] = 0.0;
        self.x[i] = 0.0;
        self.y[i] = 0.0;
        self.vx[i] = 0.0;
        self.vy[i] = 0.0;
    }

    fn merge_overlaps(&mut self, rules: &GameRules, events: &mut Vec<MergeEvent>) {
        let n = self.len();
        let mut parent: Vec<u32> = (0..n as u32).collect();
        fn find(parent: &mut [u32], mut a: u32) -> u32 {
            while parent[a as usize] != a {
                parent[a as usize] = parent[parent[a as usize] as usize];
                a = parent[a as usize];
            }
            a
        }
        let mut any = false;
        for i in 0..n {
            if !self.hit[i] {
                continue;
            }
            for j in (i + 1)..n {
                if !self.hit[j] || !self.alive[j] {
                    continue;
                }
                let dx = self.x[j] - self.x[i];
                let dy = self.y[j] - self.y[i];
                let rr = self.radius[i] + self.radius[j];
                if dx * dx + dy * dy <= rr * rr {
                    let (a, b) = (find(&mut parent, i as u32), find(&mut parent, j as u32));
                    if a != b {
                        // Root is always the lowest slot, which keeps grouping order-independent.
                        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                        parent[hi as usize] = lo;
                        any = true;
                    }
                }
            }
        }
        if !any {
            return;
        }
        let mut groups: Vec<Vec<u32>> = vec![Vec::new(); n];
        for i in 0..n as u32 {
            if self.hit[i as usize] {
                let r = find(&mut parent, i);
                groups[r as usize].push(i);
            }
        }
        for members in groups.iter().filter(|m| m.len() >= 2) {
            // Members are in ascending slot order, so every sum below has a fixed order.
            let mut keep = members[0] as usize;
            let (mut total, mut sum_abs) = (0.0, 0.0);
            let (mut px, mut py, mut wx, mut wy, mut vol) = (0.0, 0.0, 0.0, 0.0, 0.0);
            let mut r_max: f64 = 0.0;
            for &m in members {
                let i = m as usize;
                let mass = self.mass[i];
                let abs = mass.abs();
                if abs > self.mass[keep].abs() {
                    keep = i;
                }
                total += mass;
                sum_abs += abs;
                px += self.vx[i] * mass;
                py += self.vy[i] * mass;
                wx += self.x[i] * abs;
                wy += self.y[i] * abs;
                let r3 = self.radius[i] * self.radius[i] * self.radius[i];
                vol += if mass < 0.0 { -r3 } else { r3 };
                r_max = r_max.max(self.radius[i]);
            }
            if sum_abs <= 0.0 || total.abs() <= rules.annihilate_frac * sum_abs {
                for &m in members {
                    self.kill(m as usize);
                }
                events.push(MergeEvent { survivor: None, absorbed: members.clone() });
                continue;
            }
            let (nx, ny) = (wx / sum_abs, wy / sum_abs);
            let (nvx, nvy) = (px / total, py / total);
            let radius = math::cbrt(vol.abs()).max(0.25 * r_max);
            let absorbed: Vec<u32> = members.iter().copied().filter(|m| *m as usize != keep).collect();
            for &m in &absorbed {
                self.kill(m as usize);
            }
            self.mass[keep] = total;
            self.radius[keep] = radius;
            self.x[keep] = nx;
            self.y[keep] = ny;
            self.vx[keep] = nvx;
            self.vy[keep] = nvy;
            events.push(MergeEvent { survivor: Some(keep as u32), absorbed });
        }
    }

    /// Hash of the full dynamic state. Two machines that agree on this agree on everything.
    pub fn hash(&self) -> u64 {
        let mut h = Xxh3::new();
        h.update(&self.tick.to_le_bytes());
        for i in 0..self.len() {
            if !self.alive[i] {
                continue;
            }
            h.update(&(i as u32).to_le_bytes());
            for v in [self.x[i], self.y[i], self.vx[i], self.vy[i], self.mass[i], self.radius[i]] {
                h.update(&v.to_bits().to_le_bytes());
            }
        }
        h.digest()
    }

    /// Kinetic plus (softened) potential energy. Diagnostics and tests only.
    pub fn energy(&self, rules: &GameRules) -> f64 {
        let n = self.len();
        let eps2 = rules.softening * rules.softening;
        let mut e = 0.0;
        for i in 0..n {
            e += 0.5 * self.mass[i] * (self.vx[i] * self.vx[i] + self.vy[i] * self.vy[i]);
            for j in (i + 1)..n {
                let dx = self.x[j] - self.x[i];
                let dy = self.y[j] - self.y[i];
                e -= rules.g * self.mass[i] * self.mass[j] / (dx * dx + dy * dy + eps2).sqrt();
            }
        }
        e
    }

    pub fn momentum(&self) -> (f64, f64) {
        let (mut px, mut py) = (0.0, 0.0);
        for i in 0..self.len() {
            px += self.mass[i] * self.vx[i];
            py += self.mass[i] * self.vy[i];
        }
        (px, py)
    }

    pub fn total_mass(&self) -> f64 {
        self.mass.iter().sum()
    }
}

impl MassiveState {
    /// Displace a body by hand (tests, tooling). Invalidates the cached accelerations.
    pub fn nudge(&mut self, slot: usize, dx: f64, dy: f64) {
        self.x[slot] += dx;
        self.y[slot] += dy;
        self.acc_valid = false;
    }
}

/// Centre-of-mass frame of the whole system, for deciding which bodies have left it for good.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemFrame {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    /// Signed total mass.
    pub mass: f64,
}

impl SystemFrame {
    /// Sums run in slot order, so every machine gets the same bits.
    pub fn of(view: &MassiveView) -> Self {
        let (mut w, mut f) = (0.0, SystemFrame::default());
        for j in 0..view.x.len() {
            if !view.alive[j] {
                continue;
            }
            let a = view.mass[j].abs();
            w += a;
            f.x += view.x[j] * a;
            f.y += view.y[j] * a;
            f.vx += view.vx[j] * a;
            f.vy += view.vy[j] * a;
            f.mass += view.mass[j];
        }
        if w > 0.0 {
            f.x /= w;
            f.y /= w;
            f.vx /= w;
            f.vy /= w;
        }
        f
    }

    /// Distance from the barycentre, and whether the body is leaving for good: moving
    /// outwards faster than the escape speed of everything else combined.
    pub fn escape_state(&self, view: &MassiveView, j: usize, g: f64) -> (f64, bool) {
        let (dx, dy) = (view.x[j] - self.x, view.y[j] - self.y);
        let (vx, vy) = (view.vx[j] - self.vx, view.vy[j] - self.vy);
        let r = (dx * dx + dy * dy).sqrt();
        if r <= 0.0 {
            return (r, false);
        }
        let others = self.mass - view.mass[j];
        let outgoing = dx * vx + dy * vy > 0.0;
        let unbound = 0.5 * (vx * vx + vy * vy) > g * others / r;
        (r, outgoing && unbound)
    }
}

impl MassiveState {
    /// Positions, velocities and masses only: the acceleration slices may be stale.
    pub fn kinematics(&self) -> MassiveView<'_> {
        MassiveView {
            x: &self.x,
            y: &self.y,
            vx: &self.vx,
            vy: &self.vy,
            ax: &self.ax,
            ay: &self.ay,
            mass: &self.mass,
            radius: &self.radius,
            alive: &self.alive,
        }
    }

    /// Drop bodies that are beyond `rules.escape_radius` and never coming back.
    fn remove_escapers(&mut self, rules: &GameRules, events: &mut Vec<MergeEvent>) {
        let gone: Vec<usize> = {
            let view = self.kinematics();
            let frame = SystemFrame::of(&view);
            (0..self.len())
                .filter(|&j| self.alive[j])
                .filter(|&j| {
                    let (r, escaping) = frame.escape_state(&view, j, rules.g);
                    escaping && r > rules.escape_radius
                })
                .collect()
        };
        for j in gone {
            self.kill(j);
            events.push(MergeEvent { survivor: None, absorbed: vec![j as u32] });
        }
    }
}
