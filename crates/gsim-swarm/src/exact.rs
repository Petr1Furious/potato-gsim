//! A handful of bodies deserve better than the machinery built for a million: every pair
//! summed in double precision, advanced with a fourth-order symplectic integrator. This is
//! what keeps the few planets left at the end of a collapse on clean orbits.

use crate::engine::{Bodies, Merge, HINT_MARGIN, ORBIT_FRACTION};

/// Yoshida's fourth-order composition of three leapfrog steps.
const W1: f64 = 1.351_207_191_959_657_8;
const W0: f64 = -1.702_414_383_919_315_3;

/// Accelerations of every body. With `watch`, also which pairs overlap and the square of
/// the fastest rate (rad/s) at which a bound pair turns, among those faster than `watch`.
fn accelerate(b: &Bodies, g: f64, eps2: f64, ax: &mut [f64], ay: &mut [f64], watch: Option<f64>) -> (Vec<(usize, usize)>, f64) {
    let n = b.len();
    ax.fill(0.0);
    ay.fill(0.0);
    let (mut pairs, mut omega2) = (Vec::new(), 0.0f64);
    for i in 0..n {
        for j in i + 1..n {
            let (dx, dy) = (b.x[j] - b.x[i], b.y[j] - b.y[i]);
            let d2 = dx * dx + dy * dy;
            let soft = d2 + eps2;
            let inv3 = 1.0 / (soft * soft.sqrt());
            let (fi, fj) = (g * b.m[j] as f64 * inv3, g * b.m[i] as f64 * inv3);
            ax[i] += dx * fi;
            ay[i] += dy * fi;
            ax[j] -= dx * fj;
            ay[j] -= dy * fj;
            let Some(limit) = watch else { continue };
            let reach = (b.r[i] + b.r[j]) as f64;
            if d2 < reach * reach {
                pairs.push((i, j));
                continue;
            }
            let pull = g * (b.m[i] + b.m[j]) as f64 / d2.sqrt();
            let rate2 = pull / d2;
            // Bound to each other, not just passing.
            if rate2 > limit && (b.vx[j] - b.vx[i]).powi(2) + (b.vy[j] - b.vy[i]).powi(2) < 2.0 * pull {
                omega2 = omega2.max(rate2);
            }
        }
    }
    (pairs, omega2)
}

/// Advance the world by `dt`. Returns the merges and the fastest bound rate squared, as
/// [`crate::Engine`] reports it (`dt_hint` is the step the caller would like next).
pub fn step(b: &mut Bodies, dt: f64, g: f64, softening: f64, dt_hint: f64) -> (Vec<Merge>, f64) {
    b.retain(|_, _| true);
    let n = b.len();
    let eps2 = softening * softening;
    let (mut ax, mut ay) = (vec![0.0; n], vec![0.0; n]);
    let drift = |b: &mut Bodies, h: f64| {
        for i in 0..n {
            b.x[i] += b.vx[i] * h;
            b.y[i] += b.vy[i] * h;
        }
    };
    let kicks = [W1, W0, W1];
    let drifts = [0.5 * W1, 0.5 * (W0 + W1), 0.5 * (W0 + W1), 0.5 * W1];
    for k in 0..3 {
        drift(b, drifts[k] * dt);
        accelerate(b, g, eps2, &mut ax, &mut ay, None);
        for i in 0..n {
            b.vx[i] += ax[i] * kicks[k] * dt;
            b.vy[i] += ay[i] * kicks[k] * dt;
        }
    }
    drift(b, drifts[3] * dt);
    // Once more at the new positions: for whoever reads the accelerations, to find what
    // now overlaps, and to see how tight the tightest orbit is.
    let limit = if dt_hint > 0.0 { (ORBIT_FRACTION / (HINT_MARGIN * dt_hint)).powi(2) } else { f64::MAX };
    let (pairs, omega2) = accelerate(b, g, eps2, &mut ax, &mut ay, Some(limit));
    for i in 0..n {
        (b.ax[i], b.ay[i]) = (ax[i] as f32, ay[i] as f32);
    }
    let mut merges = Vec::new();
    for (i, j) in pairs {
        if b.m[i] <= 0.0 || b.m[j] <= 0.0 {
            continue;
        }
        // The same rule as in a crowd: mass and momentum kept, volumes added, the heavier lives.
        let (s, a) = if b.m[j] > b.m[i] { (j, i) } else { (i, j) };
        let (ms, ma) = (b.m[s] as f64, b.m[a] as f64);
        let inv = 1.0 / (ms + ma);
        b.x[s] = (b.x[s] * ms + b.x[a] * ma) * inv;
        b.y[s] = (b.y[s] * ms + b.y[a] * ma) * inv;
        b.vx[s] = (b.vx[s] * ms + b.vx[a] * ma) * inv;
        b.vy[s] = (b.vy[s] * ms + b.vy[a] * ma) * inv;
        b.r[s] = ((b.r[s] as f64).powi(3) + (b.r[a] as f64).powi(3)).cbrt() as f32;
        b.m[s] = (ms + ma) as f32;
        merges.push(Merge { x: b.x[s], y: b.y[s], vx: b.vx[s], vy: b.vy[s], mass: b.m[a], survivor: b.id[s], absorbed: b.id[a] });
        b.m[a] = 0.0;
        b.r[a] = 0.0;
    }
    (merges, omega2)
}
