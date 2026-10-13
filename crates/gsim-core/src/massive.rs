//! Gravitating bodies. Their evolution depends on nothing but their own previous state,
//! so it is a pure function of (initial state, tick) and never needs to be rolled back.

use crate::wide::{total, widest, Wide, LANES};
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
    work: Work,
}

/// From this many bodies on, the pieces of a force pass are shared out between threads.
/// (Below it one thread is quick enough, and waking the others costs more processor time
/// than they save.)
const PARALLEL_MIN_BODIES: usize = 2000;
/// A force pass is cut into pieces of this many bodies.
const ROWS: usize = 64;

/// Buffers a force pass works in, kept from one to the next.
#[derive(Clone, Debug, Default)]
struct Work {
    /// What each piece adds to each body's acceleration.
    x: Vec<f64>,
    y: Vec<f64>,
    /// A body's radius, or (for one that is gone) what makes every comparison false.
    reach: Vec<f64>,
}

/// What a force pass reads.
struct Sources<'a> {
    x: &'a [f64],
    y: &'a [f64],
    mass: &'a [f64],
    reach: &'a [f64],
    alive: &'a [bool],
    g: f64,
    eps2: f64,
}

/// What two bodies `(dx, dy)` apart do to each other: the acceleration of the first, that
/// of the second (to be taken away from it), and whether they overlap.
#[inline(always)]
fn pair<W: Wide>(dx: W, dy: W, reach: W, mass_i: W, mass_j: W, g: W, eps2: W) -> (W, W, W, W, W::Mask) {
    let d2 = dx * dx + dy * dy;
    // (Softening keeps the denominator positive.)
    let inv = W::splat(1.0) / (d2 + eps2).sqrt();
    let t = g * inv * inv * inv;
    let (fi, fj) = (t * mass_j, t * mass_i);
    (dx * fi, dy * fi, dx * fj, dy * fj, d2.le(reach * reach))
}

/// One piece of a force pass: the bodies from `first` on, [`ROWS`] of them, each with every
/// body after it. Each pair is worked out once and pulls both ways.
///
/// `ax` and `ay` end up holding what these pairs add to every body's acceleration (nothing
/// for the bodies before `first`, which are left alone), and `hits` the bodies found
/// overlapping.
#[inline(always)]
fn piece_body<L: Wide>(s: &Sources, first: usize, ax: &mut [f64], ay: &mut [f64], hits: &mut Vec<u32>) {
    let n = s.x.len();
    ax[first..].fill(0.0);
    ay[first..].fill(0.0);
    for i in first..(first + ROWS).min(n) {
        if !s.alive[i] {
            continue;
        }
        let (xi, yi, mi, ri) = (s.x[i], s.y[i], s.mass[i], s.reach[i]);
        // The body against those from `j` on: one of them, or eight. Returns what they do
        // to it, having taken from them what it does to them.
        #[inline(always)]
        fn meet<W: Wide>(s: &Sources, at: (f64, f64, f64, f64), j: usize, ax: &mut [f64], ay: &mut [f64]) -> (W, W, bool) {
            let (xi, yi, mi, ri) = (W::splat(at.0), W::splat(at.1), W::splat(at.2), W::splat(at.3));
            let (dx, dy) = (W::load(&s.x[j..]) - xi, W::load(&s.y[j..]) - yi);
            let (ix, iy, jx, jy, close) = pair(dx, dy, ri + W::load(&s.reach[j..]), mi, W::load(&s.mass[j..]), W::splat(s.g), W::splat(s.eps2));
            (W::load(&ax[j..]) - jx).store(&mut ax[j..]);
            (W::load(&ay[j..]) - jy).store(&mut ay[j..]);
            (ix, iy, W::any(close))
        }
        let at = (xi, yi, mi, ri);
        let (mut sx, mut sy) = ([0.0f64; LANES], [0.0f64; LANES]);
        let mut near = false;
        // Body `j` is always on lane `j % LANES`: singly up to a whole number of lanes, then
        // all lanes at once, then singly again.
        let whole = (i + 1).next_multiple_of(LANES).min(n);
        for j in i + 1..whole {
            let (ix, iy, close) = meet::<f64>(s, at, j, ax, ay);
            sx[j % LANES] += ix;
            sy[j % LANES] += iy;
            near |= close;
        }
        let (mut wx, mut wy) = (L::load(&sx), L::load(&sy));
        let mut j = whole;
        while j + LANES <= n {
            let (ix, iy, close) = meet::<L>(s, at, j, ax, ay);
            wx = wx + ix;
            wy = wy + iy;
            near |= close;
            j += LANES;
        }
        wx.store(&mut sx);
        wy.store(&mut sy);
        for j in j..n {
            let (ix, iy, close) = meet::<f64>(s, at, j, ax, ay);
            sx[j % LANES] += ix;
            sy[j % LANES] += iy;
            near |= close;
        }
        ax[i] += total(&sx);
        ay[i] += total(&sy);
        if near {
            // Rare: which ones, then.
            for j in i + 1..n {
                let (dx, dy, reach) = (s.x[j] - xi, s.y[j] - yi, ri + s.reach[j]);
                if dx * dx + dy * dy <= reach * reach {
                    hits.extend([i as u32, j as u32]);
                }
            }
        }
    }
}

widest! {
    fn piece(s: &Sources, first: usize, ax: &mut [f64], ay: &mut [f64], hits: &mut Vec<u32>) = piece_body
}

/// Acceleration on every live body plus a per-body "overlaps something" flag.
///
/// The pass is cut into pieces of [`ROWS`] bodies, each worked through serially into sums
/// of its own, and the pieces are added up in order: the result does not depend on how many
/// threads there are, or on whether there are any.
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
    work: &mut Work,
) {
    let n = x.len();
    out_ax.fill(0.0);
    out_ay.fill(0.0);
    hit.fill(false);
    if n == 0 {
        return;
    }
    let pieces = n.div_ceil(ROWS);
    work.x.resize(pieces * n, 0.0);
    work.y.resize(pieces * n, 0.0);
    work.reach.clear();
    work.reach.extend(radius.iter().zip(alive).map(|(r, alive)| if *alive { *r } else { f64::NAN }));
    let s = Sources { x, y, mass, reach: &work.reach, alive, g, eps2 };
    let one = |(k, (ax, ay)): (usize, (&mut [f64], &mut [f64]))| {
        let mut hits = Vec::new();
        piece(&s, k * ROWS, ax, ay, &mut hits);
        hits
    };
    let hits: Vec<Vec<u32>> = if n >= PARALLEL_MIN_BODIES {
        work.x.par_chunks_mut(n).zip(work.y.par_chunks_mut(n)).enumerate().map(one).collect()
    } else {
        work.x.chunks_mut(n).zip(work.y.chunks_mut(n)).enumerate().map(one).collect()
    };
    for (k, (ax, ay)) in work.x.chunks(n).zip(work.y.chunks(n)).enumerate() {
        for j in k * ROWS..n {
            out_ax[j] += ax[j];
            out_ay[j] += ay[j];
        }
    }
    for j in 0..n {
        if !alive[j] {
            (out_ax[j], out_ay[j]) = (0.0, 0.0);
        }
    }
    for i in hits.into_iter().flatten() {
        hit[i as usize] = true;
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
            work: Work::default(),
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
            work: Work::default(),
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
            &mut self.ax, &mut self.ay, &mut self.hit, &mut self.work,
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
            &mut self.bx, &mut self.by, &mut self.hit, &mut self.work,
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
