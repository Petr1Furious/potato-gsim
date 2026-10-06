//! Force kernels: the pull of a list of sources on a group of targets, many pairs per
//! instruction. Everything here is in the tree's scaled, group-local single precision.

use crate::simd::{Plain, Simd};

/// Source lists are padded to a multiple of this many entries (the widest vector).
pub const PAD: usize = 16;
/// Where padding entries sit: far from any target, with zero mass and radius.
pub const PAD_X: f32 = 1.0e3;

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

/// Distant cells acting on a group as a point mass plus quadrupole.
pub struct Far<'a> {
    pub x: &'a [f32],
    pub y: &'a [f32],
    pub m: &'a [f32],
    pub qxx: &'a [f32],
    pub qxy: &'a [f32],
    pub qyy: &'a [f32],
}

/// Four targets share each load of sources; the last pass repeats a target to fill up.
#[inline(always)]
unsafe fn near_impl<V: Simd>(tx: &[f32], ty: &[f32], tr: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Near, eps2: f32, hit: &mut Vec<u32>) {
    let e = V::splat(eps2);
    let zero = V::splat(0.0);
    let ns = s.x.len();
    let mut i = 0;
    while i < tx.len() {
        let w = (tx.len() - i).min(4);
        let at = |t: usize| i + t.min(w - 1);
        let x = [V::splat(tx[at(0)]), V::splat(tx[at(1)]), V::splat(tx[at(2)]), V::splat(tx[at(3)])];
        let y = [V::splat(ty[at(0)]), V::splat(ty[at(1)]), V::splat(ty[at(2)]), V::splat(ty[at(3)])];
        let r = [V::splat(tr[at(0)]), V::splat(tr[at(1)]), V::splat(tr[at(2)]), V::splat(tr[at(3)])];
        let mut sx = [zero; 4];
        let mut sy = [zero; 4];
        let mut touch = [false; 4];
        let mut k = 0;
        while k < ns {
            let jx = V::load(s.x.as_ptr().add(k));
            let jy = V::load(s.y.as_ptr().add(k));
            let jm = V::load(s.m.as_ptr().add(k));
            let jr = V::load(s.r.as_ptr().add(k));
            for t in 0..4 {
                let dx = jx.sub(x[t]);
                let dy = jy.sub(y[t]);
                let d2 = dx.mul_add(dx, dy.mul(dy));
                // Closer than the two radii, and not the body itself (distance zero).
                let reach = jr.add(r[t]);
                touch[t] |= d2.any_inside(reach.mul(reach));
                let inv = d2.add(e).rsqrt();
                let f = jm.mul(inv.mul(inv.mul(inv)));
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
        }
        i += w;
    }
}

/// a = m d / r^3 - (Q d) / r^5 + 2.5 (d.Q.d) d / r^7, with d from the target to the cell.
#[inline(always)]
unsafe fn far_impl<V: Simd>(tx: &[f32], ty: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Far, eps2: f32) {
    let e = V::splat(eps2);
    let c25 = V::splat(2.5);
    let zero = V::splat(0.0);
    let ns = s.x.len();
    let mut i = 0;
    while i < tx.len() {
        let w = (tx.len() - i).min(2);
        let at = |t: usize| i + t.min(w - 1);
        let x = [V::splat(tx[at(0)]), V::splat(tx[at(1)])];
        let y = [V::splat(ty[at(0)]), V::splat(ty[at(1)])];
        let mut sx = [zero; 2];
        let mut sy = [zero; 2];
        let mut k = 0;
        while k < ns {
            let jx = V::load(s.x.as_ptr().add(k));
            let jy = V::load(s.y.as_ptr().add(k));
            let jm = V::load(s.m.as_ptr().add(k));
            let qxx = V::load(s.qxx.as_ptr().add(k));
            let qxy = V::load(s.qxy.as_ptr().add(k));
            let qyy = V::load(s.qyy.as_ptr().add(k));
            for t in 0..2 {
                let dx = jx.sub(x[t]);
                let dy = jy.sub(y[t]);
                let r2 = dx.mul_add(dx, dy.mul_add(dy, e));
                let inv = r2.rsqrt();
                let inv2 = inv.mul(inv);
                let inv3 = inv2.mul(inv);
                let inv5 = inv3.mul(inv2);
                let qdx = qxx.mul_add(dx, qxy.mul(dy));
                let qdy = qxy.mul_add(dx, qyy.mul(dy));
                let dqd = dx.mul_add(qdx, dy.mul(qdy));
                // Radial factor m / r^3 + 2.5 (d.Q.d) / r^7, ordered to stay within range.
                let radial = c25.mul(dqd.mul(inv2)).mul_add(inv5, jm.mul(inv3));
                sx[t] = sx[t].add(qdx.neg_mul_add(inv5, dx.mul(radial)));
                sy[t] = sy[t].add(qdy.neg_mul_add(inv5, dy.mul(radial)));
            }
            k += V::LANES;
        }
        for t in 0..w {
            ax[i + t] += sx[t].sum();
            ay[i + t] += sy[t].sum();
        }
        i += w;
    }
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::*;
    use crate::simd::x86::{Avx2, Avx512};

    #[target_feature(enable = "avx512f")]
    pub unsafe fn near_avx512(tx: &[f32], ty: &[f32], tr: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Near, eps2: f32, hit: &mut Vec<u32>) {
        near_impl::<Avx512>(tx, ty, tr, ax, ay, s, eps2, hit)
    }
    #[target_feature(enable = "avx512f")]
    pub unsafe fn far_avx512(tx: &[f32], ty: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Far, eps2: f32) {
        far_impl::<Avx512>(tx, ty, ax, ay, s, eps2)
    }
    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn near_avx2(tx: &[f32], ty: &[f32], tr: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Near, eps2: f32, hit: &mut Vec<u32>) {
        near_impl::<Avx2>(tx, ty, tr, ax, ay, s, eps2, hit)
    }
    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn far_avx2(tx: &[f32], ty: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Far, eps2: f32) {
        far_impl::<Avx2>(tx, ty, ax, ay, s, eps2)
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use super::*;
    use crate::simd::arm::Neon;

    #[target_feature(enable = "neon")]
    pub unsafe fn near_neon(tx: &[f32], ty: &[f32], tr: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Near, eps2: f32, hit: &mut Vec<u32>) {
        near_impl::<Neon>(tx, ty, tr, ax, ay, s, eps2, hit)
    }
    #[target_feature(enable = "neon")]
    pub unsafe fn far_neon(tx: &[f32], ty: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Far, eps2: f32) {
        far_impl::<Neon>(tx, ty, ax, ay, s, eps2)
    }
}

/// Accelerations of the targets from `s` (overwrites `ax`/`ay`), in scaled units without G.
/// Targets that overlap a source are appended to `hit` by their index in the group.
///
/// `level` must come from [`Level::detect`] or [`Level::available`]; source lists must be
/// padded to a multiple of [`PAD`].
pub fn near(level: Level, tx: &[f32], ty: &[f32], tr: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Near, eps2: f32, hit: &mut Vec<u32>) {
    assert!(s.x.len().is_multiple_of(PAD) && s.y.len() == s.x.len() && s.m.len() == s.x.len() && s.r.len() == s.x.len());
    assert!(ty.len() == tx.len() && tr.len() == tx.len() && ax.len() == tx.len() && ay.len() == tx.len());
    // SAFETY: lengths checked above; the level was detected on this CPU.
    unsafe {
        match level {
            #[cfg(target_arch = "x86_64")]
            Level::Avx512 => x86::near_avx512(tx, ty, tr, ax, ay, s, eps2, hit),
            #[cfg(target_arch = "x86_64")]
            Level::Avx2 => x86::near_avx2(tx, ty, tr, ax, ay, s, eps2, hit),
            #[cfg(target_arch = "aarch64")]
            Level::Neon => arm::near_neon(tx, ty, tr, ax, ay, s, eps2, hit),
            _ => near_impl::<Plain>(tx, ty, tr, ax, ay, s, eps2, hit),
        }
    }
}

/// Adds the pull of distant cells to `ax`/`ay`. Same contract as [`near`].
pub fn far(level: Level, tx: &[f32], ty: &[f32], ax: &mut [f32], ay: &mut [f32], s: &Far, eps2: f32) {
    let n = s.x.len();
    assert!(n.is_multiple_of(PAD) && s.y.len() == n && s.m.len() == n && s.qxx.len() == n && s.qxy.len() == n && s.qyy.len() == n);
    assert!(ty.len() == tx.len() && ax.len() == tx.len() && ay.len() == tx.len());
    // SAFETY: as in `near`.
    unsafe {
        match level {
            #[cfg(target_arch = "x86_64")]
            Level::Avx512 => x86::far_avx512(tx, ty, ax, ay, s, eps2),
            #[cfg(target_arch = "x86_64")]
            Level::Avx2 => x86::far_avx2(tx, ty, ax, ay, s, eps2),
            #[cfg(target_arch = "aarch64")]
            Level::Neon => arm::far_neon(tx, ty, ax, ay, s, eps2),
            _ => far_impl::<Plain>(tx, ty, ax, ay, s, eps2),
        }
    }
}
