//! Massless test particles: they feel the massive bodies but exert nothing, so each one can
//! be integrated, predicted and rolled back independently of everything else.

use crate::{GameRules, MassiveView};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Particle {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
}

/// Reusable buffers (particle position relative to each body at the previous sub-step).
#[derive(Default)]
pub struct Scratch {
    rx: Vec<f64>,
    ry: Vec<f64>,
}

/// Number of sub-steps for this tick: the smallest power of two that resolves both the
/// strongest gravitational time scale and the fastest relative fly-by. Uses only
/// `* / sqrt` and comparisons, so every machine picks the same value.
pub fn substeps(p: &Particle, view: &MassiveView, rules: &GameRules) -> u32 {
    let eps2 = rules.particle_softening * rules.particle_softening;
    let inv_eta2 = 1.0 / (rules.substep_eta * rules.substep_eta);
    let inv_kappa2 = 1.0 / (rules.substep_kappa * rules.substep_kappa);
    let mut need: f64 = 0.0;
    for j in 0..view.x.len() {
        if !view.alive[j] {
            continue;
        }
        let dx = view.x[j] - p.x;
        let dy = view.y[j] - p.y;
        let d2 = dx * dx + dy * dy + eps2;
        let d = d2.sqrt();
        let omega2 = rules.g * view.mass[j].abs() / (d2 * d);
        let rvx = p.vx - view.vx[j];
        let rvy = p.vy - view.vy[j];
        let fly2 = (rvx * rvx + rvy * rvy) / d2;
        need = need.max(omega2 * inv_eta2).max(fly2 * inv_kappa2);
    }
    let target = rules.dt * rules.dt * need;
    let mut n: u32 = 1;
    while n < rules.max_substeps && ((n as f64) * (n as f64)) < target {
        n *= 2;
    }
    n
}

/// Gravity on the particle at `tau` seconds into the tick, with body positions following
/// the same parabola the massive integrator uses. Also sweeps the particle's path relative
/// to each body since the previous call and reports the lowest slot it touched.
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
    let eps2 = rules.particle_softening * rules.particle_softening;
    let half_tau = 0.5 * tau;
    let (mut ax, mut ay) = (0.0, 0.0);
    let mut hit = None;
    for j in 0..view.x.len() {
        if !view.alive[j] {
            continue;
        }
        let bx = view.x[j] + tau * (view.vx[j] + view.ax[j] * half_tau);
        let by = view.y[j] + tau * (view.vy[j] + view.ay[j] * half_tau);
        // Particle relative to the body.
        let rx = px - bx;
        let ry = py - by;
        let d2 = rx * rx + ry * ry;
        let reach = view.radius[j] + radius;
        let reach2 = reach * reach;
        if hit.is_none() {
            let touched = if first {
                d2 <= reach2
            } else {
                let (r0x, r0y) = (scratch.rx[j], scratch.ry[j]);
                let (sx, sy) = (rx - r0x, ry - r0y);
                let ss = sx * sx + sy * sy;
                let t = if ss > 0.0 { (-(r0x * sx + r0y * sy) / ss).clamp(0.0, 1.0) } else { 0.0 };
                let (cx, cy) = (r0x + t * sx, r0y + t * sy);
                cx * cx + cy * cy <= reach2
            };
            if touched {
                hit = Some(j as u32);
            }
        }
        scratch.rx[j] = rx;
        scratch.ry[j] = ry;
        let d2s = d2 + eps2;
        let inv = 1.0 / d2s.sqrt();
        let f = rules.g * view.mass[j] * inv * inv * inv;
        ax -= rx * f;
        ay -= ry * f;
    }
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
    scratch.rx.resize(nb, 0.0);
    scratch.ry.resize(nb, 0.0);
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
