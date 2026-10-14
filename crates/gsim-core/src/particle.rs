//! Massless test particles: they feel the massive bodies but exert nothing, so each one can
//! be integrated, predicted and rolled back independently of everything else.

use crate::wide::{total, widest, Wide, LANES};
use crate::{GameRules, MassiveView};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Particle {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
}

/// Reusable buffers: the particle's position relative to each body at the previous sub-step,
/// and at the one being worked out.
#[derive(Default)]
pub struct Scratch {
    rx: Vec<f64>,
    ry: Vec<f64>,
    nx: Vec<f64>,
    ny: Vec<f64>,
}

/// Bodies as a particle meets them: one, or eight side by side.
#[derive(Clone, Copy)]
struct Met<W: Wide> {
    x: W,
    y: W,
    vx: W,
    vy: W,
    ax: W,
    ay: W,
    mass: W,
    radius: W,
    /// 1 for a body that is there, 0 for one that is gone.
    alive: W,
}

impl<W: Wide> Met<W> {
    /// Those from slot `j` on.
    #[inline(always)]
    fn at(view: &MassiveView, j: usize) -> Self {
        Self {
            x: W::load(&view.x[j..]),
            y: W::load(&view.y[j..]),
            vx: W::load(&view.vx[j..]),
            vy: W::load(&view.vy[j..]),
            ax: W::load(&view.ax[j..]),
            ay: W::load(&view.ay[j..]),
            // (What a ship feels: a base pulls it in, whatever it does to bodies.)
            mass: W::load(&view.pull[j..]),
            radius: W::load(&view.radius[j..]),
            alive: W::flags(&view.alive[j..]),
        }
    }
}

/// How a tick is cut finely enough for a particle: by how hard a body pulls, and by how
/// fast the two pass each other.
#[derive(Clone, Copy)]
struct Fineness {
    g: f64,
    eps2: f64,
    inv_eta2: f64,
    inv_kappa2: f64,
}

/// The larger of two numbers that are not NaN.
#[inline(always)]
fn larger<W: Wide>(a: W, b: W) -> W {
    W::pick(a.gt(b), a, b)
}

/// What bodies ask for. (Nothing, if they are gone.)
#[inline(always)]
fn asks<W: Wide>(p: &Particle, b: &Met<W>, f: &Fineness) -> W {
    let (dx, dy) = (b.x - W::splat(p.x), b.y - W::splat(p.y));
    let (rvx, rvy) = (W::splat(p.vx) - b.vx, W::splat(p.vy) - b.vy);
    // One over the distance squared, and over the distance.
    let inv2 = W::splat(1.0) / (dx * dx + dy * dy + W::splat(f.eps2));
    let inv = inv2.sqrt();
    let omega2 = W::splat(f.g) * b.mass.abs() * inv2 * inv;
    let fly2 = (rvx * rvx + rvy * rvy) * inv2;
    larger(omega2 * W::splat(f.inv_eta2), fly2 * W::splat(f.inv_kappa2)) * b.alive
}

#[inline(always)]
fn need_body<L: Wide>(p: &Particle, view: &MassiveView, rules: &GameRules) -> f64 {
    let f = Fineness {
        g: rules.g,
        eps2: rules.particle_softening * rules.particle_softening,
        inv_eta2: 1.0 / (rules.substep_eta * rules.substep_eta),
        inv_kappa2: 1.0 / (rules.substep_kappa * rules.substep_kappa),
    };
    let n = view.x.len();
    // (The largest of them is the same in whatever order they are taken.)
    let mut most = L::splat(0.0);
    let mut j = 0;
    while j + LANES <= n {
        most = larger(most, asks(p, &Met::<L>::at(view, j), &f));
        j += LANES;
    }
    let mut each = [0.0f64; LANES];
    most.store(&mut each);
    (j..n).map(|j| asks(p, &Met::<f64>::at(view, j), &f)).chain(each).fold(0.0, larger)
}

widest! {
    fn need(p: &Particle, view: &MassiveView, rules: &GameRules) -> f64 = need_body
}

/// Number of sub-steps for this tick: the smallest power of two that resolves both the
/// strongest gravitational time scale and the fastest relative fly-by. Uses only
/// `* / sqrt` and comparisons, so every machine picks the same value.
pub fn substeps(p: &Particle, view: &MassiveView, rules: &GameRules) -> u32 {
    let target = rules.dt * rules.dt * need(p, view, rules);
    let mut n: u32 = 1;
    while n < rules.max_substeps && ((n as f64) * (n as f64)) < target {
        n *= 2;
    }
    n
}

/// A particle at one moment of a tick, against the bodies.
#[derive(Clone, Copy)]
struct Moment {
    px: f64,
    py: f64,
    /// Seconds into the tick.
    tau: f64,
    radius: f64,
    g: f64,
    eps2: f64,
    /// The first moment of the tick: there is no earlier position to sweep from.
    first: bool,
}

/// The particle relative to bodies, their pull on it (to be taken away), and whether it
/// touched them on its way from where it was relative to them, `(r0x, r0y)`.
#[inline(always)]
fn against<W: Wide>(m: &Moment, b: &Met<W>, r0x: W, r0y: W) -> (W, W, W, W, W::Mask) {
    let (zero, tau) = (W::splat(0.0), W::splat(m.tau));
    // Bodies follow the same parabola the massive integrator uses.
    let half_tau = W::splat(0.5 * m.tau);
    let bx = b.x + tau * (b.vx + b.ax * half_tau);
    let by = b.y + tau * (b.vy + b.ay * half_tau);
    let (rx, ry) = (W::splat(m.px) - bx, W::splat(m.py) - by);
    let d2 = rx * rx + ry * ry;
    let reach = b.radius + W::splat(m.radius);
    let reach2 = reach * reach;
    let there = d2.le(reach2);
    let hit = if m.first {
        there
    } else {
        // The straight path from the earlier position: where it is nearest the body (at
        // its start, at its end, or between), and whether that is within reach. Without
        // dividing or branching: by the square of the path's length, all three ways at once.
        let (sx, sy) = (rx - r0x, ry - r0y);
        let ss = sx * sx + sy * sy;
        let along = r0x * sx + r0y * sy;
        let from2 = r0x * r0x + r0y * r0y;
        let (start, end) = (zero.le(along), ss.le(zero - along));
        let between = (from2 * ss - along * along).le(reach2 * ss);
        let inside = W::or(W::and(end, there), W::and(W::not(end), between));
        W::or(W::and(start, from2.le(reach2)), W::and(W::not(start), inside))
    };
    let inv = W::splat(1.0) / (d2 + W::splat(m.eps2)).sqrt();
    let f = W::splat(m.g) * b.mass * inv * inv * inv;
    (rx, ry, rx * f, ry * f, W::and(b.alive.gt(zero), hit))
}

/// Gravity on the particle at one moment, and whether it touched anything since the one
/// before. Leaves its position relative to each body in `scratch.nx, ny`.
#[inline(always)]
fn pulls_body<L: Wide>(m: &Moment, view: &MassiveView, scratch: &mut Scratch) -> (f64, f64, bool) {
    let n = view.x.len();
    let (mut wx, mut wy) = (L::splat(0.0), L::splat(0.0));
    let mut any = false;
    let mut j = 0;
    while j + LANES <= n {
        let (rx, ry, fx, fy, touched) = against(m, &Met::<L>::at(view, j), L::load(&scratch.rx[j..]), L::load(&scratch.ry[j..]));
        rx.store(&mut scratch.nx[j..]);
        ry.store(&mut scratch.ny[j..]);
        wx = wx + fx;
        wy = wy + fy;
        any |= L::any(touched);
        j += LANES;
    }
    let (mut sx, mut sy) = ([0.0f64; LANES], [0.0f64; LANES]);
    wx.store(&mut sx);
    wy.store(&mut sy);
    for j in j..n {
        let (rx, ry, fx, fy, touched) = against(m, &Met::<f64>::at(view, j), scratch.rx[j], scratch.ry[j]);
        (scratch.nx[j], scratch.ny[j]) = (rx, ry);
        sx[j % LANES] += fx;
        sy[j % LANES] += fy;
        any |= touched;
    }
    (-total(&sx), -total(&sy), any)
}

widest! {
    fn pulls(m: &Moment, view: &MassiveView, scratch: &mut Scratch) -> (f64, f64, bool) = pulls_body
}

/// Gravity on the particle at `tau` seconds into the tick. Also sweeps the particle's path
/// relative to each body since the previous call and reports the lowest slot it touched.
#[allow(clippy::too_many_arguments)]
fn accel_and_sweep(
    px: f64,
    py: f64,
    tau: f64,
    radius: f64,
    view: &MassiveView,
    rules: &GameRules,
    scratch: &mut Scratch,
    first: bool,
) -> (f64, f64, Option<u32>) {
    let m = Moment { px, py, tau, radius, g: rules.g, eps2: rules.particle_softening * rules.particle_softening, first };
    let (ax, ay, any) = pulls(&m, view, scratch);
    // Rare: which one, then.
    let hit = if any { (0..view.x.len()).find(|&j| against(&m, &Met::<f64>::at(view, j), scratch.rx[j], scratch.ry[j]).4).map(|j| j as u32) } else { None };
    std::mem::swap(&mut scratch.rx, &mut scratch.nx);
    std::mem::swap(&mut scratch.ry, &mut scratch.ny);
    (ax, ay, hit)
}

/// Integrate one tick with a constant extra acceleration (`thrust`). Returns the slot of the
/// body the particle ran into, if any; the particle is then left at the point of contact.
pub fn step_particle(
    p: &mut Particle,
    thrust: (f64, f64),
    radius: f64,
    view: &MassiveView,
    rules: &GameRules,
    scratch: &mut Scratch,
) -> Option<u32> {
    let nb = view.x.len();
    for list in [&mut scratch.rx, &mut scratch.ry, &mut scratch.nx, &mut scratch.ny] {
        list.resize(nb, 0.0);
    }
    let n = substeps(p, view, rules);
    let h = rules.dt / n as f64;
    let half_h = 0.5 * h;
    let (mut ax, mut ay, hit) = accel_and_sweep(p.x, p.y, 0.0, radius, view, rules, scratch, true);
    if hit.is_some() {
        return hit;
    }
    for k in 0..n {
        let tau = if k + 1 == n { rules.dt } else { h * (k + 1) as f64 };
        let vhx = p.vx + (ax + thrust.0) * half_h;
        let vhy = p.vy + (ay + thrust.1) * half_h;
        let nx = p.x + vhx * h;
        let ny = p.y + vhy * h;
        let (ax1, ay1, hit) = accel_and_sweep(nx, ny, tau, radius, view, rules, scratch, false);
        p.x = nx;
        p.y = ny;
        p.vx = vhx + (ax1 + thrust.0) * half_h;
        p.vy = vhy + (ay1 + thrust.1) * half_h;
        if hit.is_some() {
            return hit;
        }
        ax = ax1;
        ay = ay1;
    }
    None
}

/// Closest approach of two points moving linearly over one tick (`a0→a1`, `b0→b1`):
/// returns the squared minimum distance. Used for shell proximity fuses.
pub fn swept_min_dist2(a0: (f64, f64), a1: (f64, f64), b0: (f64, f64), b1: (f64, f64)) -> f64 {
    let (r0x, r0y) = (a0.0 - b0.0, a0.1 - b0.1);
    let (r1x, r1y) = (a1.0 - b1.0, a1.1 - b1.1);
    let (sx, sy) = (r1x - r0x, r1y - r0y);
    let ss = sx * sx + sy * sy;
    let t = if ss > 0.0 { (-(r0x * sx + r0y * sy) / ss).clamp(0.0, 1.0) } else { 0.0 };
    let (cx, cy) = (r0x + t * sx, r0y + t * sy);
    cx * cx + cy * cy
}
