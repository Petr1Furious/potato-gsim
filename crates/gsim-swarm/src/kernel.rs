//! Force kernels: the pull of a list of sources on a group of targets, many pairs per
//! instruction. Everything here is in single precision, relative to the centre of the group
//! of targets and in units of that group's own size.

use crate::simd::{Plain, Simd};

/// Source lists are padded to a multiple of this many entries (the widest vector).
pub const PAD: usize = 16;
/// Where padding entries sit: far from any target, with zero mass and radius, yet not so far
/// that 1/d^3 leaves the normal range of single precision (which is slow to compute with).
pub const PAD_X: f32 = 1.0e9;

/// Instruction set the kernels run on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Avx512,
    Avx2,
    Neon,
    Plain,
}

impl Level {
    /// The best the CPU we are running on supports.
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx512f") {
                return Level::Avx512;
            }
            if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
                return Level::Avx2;
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("neon") {
                return Level::Neon;
            }
        }
        Level::Plain
    }

    /// Every level this CPU can run, best first (for tests and the benchmark).
    pub fn available() -> Vec<Self> {
        let best = Self::detect();
        let mut out = vec![best];
        if best == Level::Avx512 {
            out.push(Level::Avx2);
        }
        if best != Level::Plain {
            out.push(Level::Plain);
        }
        out
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::Avx512 => "AVX-512",
            Level::Avx2 => "AVX2",
            Level::Neon => "NEON",
            Level::Plain => "scalar",
        }
    }
}

/// Bodies acting on a group one by one: position, mass and radius.
pub struct Near<'a> {
    pub x: &'a [f32],
    pub y: &'a [f32],
    pub m: &'a [f32],
    pub r: &'a [f32],
}

/// The bodies of the group whose pull is wanted: as in [`Near`].
pub struct Targets<'a> {
    pub x: &'a [f32],
    pub y: &'a [f32],
    pub m: &'a [f32],
    pub r: &'a [f32],
}

/// Two bodies closer to each other than this, in units of the group's size, are left out of
/// the sums like those that turn too fast: single precision, which puts a body to within a
/// ten-millionth of that size, no longer says well enough how far apart they are.
pub const CLOSE: f32 = 1.0e-3;

/// Distant cells acting on a group as a point mass plus quadrupole.
pub struct Far<'a> {
    pub x: &'a [f32],
    pub y: &'a [f32],
    pub m: &'a [f32],
    pub qxx: &'a [f32],
    pub qxy: &'a [f32],
    pub qyy: &'a [f32],
}

/// What the kernels leave out of the sums: for one target (by its index in the group), the
/// sources among those from `first` on whose bit is set.
pub type Left = (u32, u32, u32);

/// Four targets share each load of sources; the last pass repeats a target to fill up.
#[inline(always)]
unsafe fn near_impl<V: Simd>(t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Near, eps2: f32, cut: f32, hit: &mut Vec<u32>, left: &mut Vec<Left>) {
    let e = V::splat(eps2);
    let cut = V::splat(cut);
    let close = V::splat(CLOSE * CLOSE);
    let zero = V::splat(0.0);
    let ns = s.x.len();
    let mut i = 0;
    while i < t.x.len() {
        let w = (t.x.len() - i).min(4);
        let at = |k: usize| i + k.min(w - 1);
        let four = |v: &[f32]| [V::splat(v[at(0)]), V::splat(v[at(1)]), V::splat(v[at(2)]), V::splat(v[at(3)])];
        let (x, y, r, m) = (four(t.x), four(t.y), four(t.r), four(t.m));
        let mut sx = [zero; 4];
        let mut sy = [zero; 4];
        let mut touch = [false; 4];
        let mut swing = [zero; 4];
        let mut k = 0;
        while k < ns {
            let jx = V::load(s.x.as_ptr().add(k));
            let jy = V::load(s.y.as_ptr().add(k));
            let jm = V::load(s.m.as_ptr().add(k));
            let jr = V::load(s.r.as_ptr().add(k));
            for t in 0..w {
                let dx = jx.sub(x[t]);
                let dy = jy.sub(y[t]);
                let d2 = dx.mul_add(dx, dy.mul(dy));
                // Closer than the two radii, and not the body itself (distance zero).
                let reach = jr.add(r[t]);
                touch[t] |= d2.any_inside(reach.mul(reach));
                let inv = d2.add(e).rsqrt();
                let inv3 = inv.mul(inv.mul(inv));
                // (m1 + m2) / d^3 is the square of the rate at which the two would circle each
                // other: keep the largest (the body itself, at distance zero, aside).
                let round = jm.add(m[t]).mul(inv3);
                swing[t] = swing[t].max_where(round, d2);
                // A pair that turns faster than `cut`, or is too close to tell how far apart,
                // is left out.
                let (f, over) = round.keep_apart(cut, d2, close, jm.mul(inv3));
                if over != 0 {
                    left.push(((i + t) as u32, k as u32, over));
                }
                sx[t] = dx.mul_add(f, sx[t]);
                sy[t] = dy.mul_add(f, sy[t]);
            }
            k += V::LANES;
        }
        for t in 0..w {
            ax[i + t] = sx[t].sum();
            ay[i + t] = sy[t].sum();
            if touch[t] {
                hit.push((i + t) as u32);
            }
            rate[i + t] = swing[t].top();
        }
        i += w;
    }
}

/// a = m d / r^3 - (Q d) / r^5 + 2.5 (d.Q.d) d / r^7, with d from the target to the cell.
#[inline(always)]
unsafe fn far_impl<V: Simd>(t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Far, eps2: f32, cut: f32, left: &mut Vec<Left>) {
    let e = V::splat(eps2);
    let cut = V::splat(cut);
    let c25 = V::splat(2.5);
    let zero = V::splat(0.0);
    let ns = s.x.len();
    let mut i = 0;
    while i < t.x.len() {
        let w = (t.x.len() - i).min(2);
        let at = |k: usize| i + k.min(w - 1);
        let two = |v: &[f32]| [V::splat(v[at(0)]), V::splat(v[at(1)])];
        let (x, y, m) = (two(t.x), two(t.y), two(t.m));
        let mut sx = [zero; 2];
        let mut sy = [zero; 2];
        let mut swing = [zero; 2];
        let mut k = 0;
        while k < ns {
            let jx = V::load(s.x.as_ptr().add(k));
            let jy = V::load(s.y.as_ptr().add(k));
            let jm = V::load(s.m.as_ptr().add(k));
            let qxx = V::load(s.qxx.as_ptr().add(k));
            let qxy = V::load(s.qxy.as_ptr().add(k));
            let qyy = V::load(s.qyy.as_ptr().add(k));
            for t in 0..w {
                let dx = jx.sub(x[t]);
                let dy = jy.sub(y[t]);
                let r2 = dx.mul_add(dx, dy.mul_add(dy, e));
                let inv = r2.rsqrt();
                let inv2 = inv.mul(inv);
                let inv3 = inv2.mul(inv);
                // As in `near`: how tightly the cell, as one lump, holds the target. Where
                // that is too tightly for the target, the lump's mass is left out (what its
                // shape adds is not).
                let round = jm.add(m[t]).mul(inv3);
                swing[t] = swing[t].max_where(round, r2);
                let (jm, over) = round.keep_le(cut, jm);
                if over != 0 {
                    left.push(((i + t) as u32, k as u32, over));
                }
                // A cell can be a great many of the group's units away, so nothing is formed
                // on the way that is much larger or smaller than the pull itself, m / r^2:
                // each power of the distance is taken off as soon as one has been put on.
                let (ux, uy) = (dx.mul(inv2), dy.mul(inv2));
                // (Q d) / r^2 and (d.Q.d) / r^2
                let qdx = qxx.mul_add(ux, qxy.mul(uy));
                let qdy = qxy.mul_add(ux, qyy.mul(uy));
                let dqd = dx.mul_add(qdx, dy.mul(qdy));
                // Along the line to the cell: (m + 2.5 (d.Q.d) / r^4) / r^2.
                let radial = c25.mul(dqd).mul_add(inv2, jm).mul(inv2);
                sx[t] = sx[t].add(qdx.mul(inv2).neg_mul_add(inv, dx.mul(inv).mul(radial)));
                sy[t] = sy[t].add(qdy.mul(inv2).neg_mul_add(inv, dy.mul(inv).mul(radial)));
            }
            k += V::LANES;
        }
        for t in 0..w {
            ax[i + t] += sx[t].sum();
            ay[i + t] += sy[t].sum();
            rate[i + t] = rate[i + t].max(swing[t].top());
        }
        i += w;
    }
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::*;
    use crate::simd::x86::{Avx2, Avx512};

    #[target_feature(enable = "avx512f")]
    pub unsafe fn near_avx512(t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Near, eps2: f32, cut: f32, hit: &mut Vec<u32>, left: &mut Vec<Left>) {
        near_impl::<Avx512>(t, ax, ay, rate, s, eps2, cut, hit, left)
    }

    #[target_feature(enable = "avx512f")]
    pub unsafe fn far_avx512(t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Far, eps2: f32, cut: f32, left: &mut Vec<Left>) {
        far_impl::<Avx512>(t, ax, ay, rate, s, eps2, cut, left)
    }

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn near_avx2(t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Near, eps2: f32, cut: f32, hit: &mut Vec<u32>, left: &mut Vec<Left>) {
        near_impl::<Avx2>(t, ax, ay, rate, s, eps2, cut, hit, left)
    }

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn far_avx2(t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Far, eps2: f32, cut: f32, left: &mut Vec<Left>) {
        far_impl::<Avx2>(t, ax, ay, rate, s, eps2, cut, left)
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use super::*;
    use crate::simd::arm::Neon;

    #[target_feature(enable = "neon")]
    pub unsafe fn near_neon(t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Near, eps2: f32, cut: f32, hit: &mut Vec<u32>, left: &mut Vec<Left>) {
        near_impl::<Neon>(t, ax, ay, rate, s, eps2, cut, hit, left)
    }

    #[target_feature(enable = "neon")]
    pub unsafe fn far_neon(t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Far, eps2: f32, cut: f32, left: &mut Vec<Left>) {
        far_impl::<Neon>(t, ax, ay, rate, s, eps2, cut, left)
    }
}

/// Accelerations of the targets from `s` (overwrites `ax`/`ay`), in scaled units without G.
///
/// Targets that overlap a source are appended to `hit` by their index in the group. `rate`
/// gets, for each target, the largest `(m1 + m2) / d^3` with any source: the square of the
/// rate at which it and whatever holds it most tightly would circle each other. A source
/// for which that is above `cut`, or which is closer than [`CLOSE`], is not added to the
/// target's sum but noted in `left` (the target itself, at distance zero, among them).
///
/// `level` must come from [`Level::detect`] or [`Level::available`]; source lists must be
/// padded to a multiple of [`PAD`].
pub fn near(level: Level, t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Near, eps2: f32, cut: f32, hit: &mut Vec<u32>, left: &mut Vec<Left>) {
    let (n, of) = (s.x.len(), t.x.len());
    assert!(n.is_multiple_of(PAD) && [s.y, s.m, s.r].iter().all(|v| v.len() == n));
    assert!([t.y, t.r, t.m, &*ax, &*ay, &*rate].iter().all(|v| v.len() == of));
    // SAFETY: lengths checked above; the level was detected on this CPU.
    unsafe {
        match level {
            #[cfg(target_arch = "x86_64")]
            Level::Avx512 => x86::near_avx512(t, ax, ay, rate, s, eps2, cut, hit, left),
            #[cfg(target_arch = "x86_64")]
            Level::Avx2 => x86::near_avx2(t, ax, ay, rate, s, eps2, cut, hit, left),
            #[cfg(target_arch = "aarch64")]
            Level::Neon => arm::near_neon(t, ax, ay, rate, s, eps2, cut, hit, left),
            _ => near_impl::<Plain>(t, ax, ay, rate, s, eps2, cut, hit, left),
        }
    }
}

/// Adds the pull of distant cells to `ax`/`ay`, and raises `rate` where one of them holds a
/// target more tightly than anything so far. A cell that holds a target above `cut` gives
/// only what its shape adds, and is noted in `left`. Same contract as [`near`].
pub fn far(level: Level, t: &Targets, ax: &mut [f32], ay: &mut [f32], rate: &mut [f32], s: &Far, eps2: f32, cut: f32, left: &mut Vec<Left>) {
    let (n, of) = (s.x.len(), t.x.len());
    assert!(n.is_multiple_of(PAD) && [s.y, s.m, s.qxx, s.qxy, s.qyy].iter().all(|v| v.len() == n));
    assert!([t.y, t.m, &*ax, &*ay, &*rate].iter().all(|v| v.len() == of));
    // SAFETY: as in `near`.
    unsafe {
        match level {
            #[cfg(target_arch = "x86_64")]
            Level::Avx512 => x86::far_avx512(t, ax, ay, rate, s, eps2, cut, left),
            #[cfg(target_arch = "x86_64")]
            Level::Avx2 => x86::far_avx2(t, ax, ay, rate, s, eps2, cut, left),
            #[cfg(target_arch = "aarch64")]
            Level::Neon => arm::far_neon(t, ax, ay, rate, s, eps2, cut, left),
            _ => far_impl::<Plain>(t, ax, ay, rate, s, eps2, cut, left),
        }
    }
}
