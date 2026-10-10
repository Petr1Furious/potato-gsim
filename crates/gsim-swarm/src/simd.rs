//! The handful of vector operations the force kernels need, for each instruction set we
//! can meet: AVX-512 (16 floats at a time), AVX2 (8), NEON (4) and plain arrays (4).
//!
//! Every method is `inline(always)` so that a kernel generic over [`Simd`] collapses into
//! straight vector code inside the `#[target_feature]` wrapper that instantiates it.

/// A pack of `LANES` single-precision floats.
///
/// # Safety
/// Methods may only be called on a CPU that has the instruction set of the implementor.
pub trait Simd: Copy {
    const LANES: usize;
    unsafe fn splat(v: f32) -> Self;
    unsafe fn load(p: *const f32) -> Self;
    unsafe fn add(self, o: Self) -> Self;
    unsafe fn sub(self, o: Self) -> Self;
    unsafe fn mul(self, o: Self) -> Self;
    /// `self * a + b`
    unsafe fn mul_add(self, a: Self, b: Self) -> Self;
    /// `b - self * a`
    unsafe fn neg_mul_add(self, a: Self, b: Self) -> Self;
    /// Roughly `1 / sqrt(self)`, good to at least four digits.
    unsafe fn rsqrt(self) -> Self;
    /// Sum of all lanes.
    unsafe fn sum(self) -> f32;
    /// Is `0 < self < hi` in any lane?
    unsafe fn any_inside(self, hi: Self) -> bool;
    /// Lane by lane the larger of `self` and `o`, where `d` is positive; `self` elsewhere.
    /// Neither may be negative.
    unsafe fn max_where(self, o: Self, d: Self) -> Self;
    /// The largest lane.
    unsafe fn top(self) -> f32;
    /// Lane by lane `value` where `self` is no more than `limit` and zero where it is more,
    /// with a bit set for each lane where it is.
    unsafe fn keep_le(self, limit: Self, value: Self) -> (Self, u32);
    /// As [`Simd::keep_le`], leaving out as well the lanes where `near` is less than `least`.
    unsafe fn keep_apart(self, limit: Self, near: Self, least: Self, value: Self) -> (Self, u32);
}

#[cfg(target_arch = "x86_64")]
/// One Newton step on an estimate `y` of `1 / sqrt(x)`: `y * (1.5 - 0.5 * x * y * y)`.
#[inline(always)]
unsafe fn refine<V: Simd>(x: V, y: V) -> V {
    let half_xyy = V::splat(0.5).mul(x).mul(y).mul(y);
    y.mul(V::splat(1.5).sub(half_xyy))
}

/// Portable fallback: four lanes the compiler may or may not vectorise.
#[derive(Clone, Copy)]
pub struct Plain([f32; 4]);

impl Plain {
    #[inline(always)]
    fn zip(self, o: Self, f: impl Fn(f32, f32) -> f32) -> Self {
        Plain([f(self.0[0], o.0[0]), f(self.0[1], o.0[1]), f(self.0[2], o.0[2]), f(self.0[3], o.0[3])])
    }
}

impl Simd for Plain {
    const LANES: usize = 4;
    #[inline(always)]
    unsafe fn splat(v: f32) -> Self {
        Plain([v; 4])
    }
    #[inline(always)]
    unsafe fn load(p: *const f32) -> Self {
        Plain(std::ptr::read_unaligned(p as *const [f32; 4]))
    }
    #[inline(always)]
    unsafe fn add(self, o: Self) -> Self {
        self.zip(o, |a, b| a + b)
    }
    #[inline(always)]
    unsafe fn sub(self, o: Self) -> Self {
        self.zip(o, |a, b| a - b)
    }
    #[inline(always)]
    unsafe fn mul(self, o: Self) -> Self {
        self.zip(o, |a, b| a * b)
    }
    #[inline(always)]
    unsafe fn mul_add(self, a: Self, b: Self) -> Self {
        self.mul(a).add(b)
    }
    #[inline(always)]
    unsafe fn neg_mul_add(self, a: Self, b: Self) -> Self {
        b.sub(self.mul(a))
    }
    #[inline(always)]
    unsafe fn rsqrt(self) -> Self {
        Plain([1.0 / self.0[0].sqrt(), 1.0 / self.0[1].sqrt(), 1.0 / self.0[2].sqrt(), 1.0 / self.0[3].sqrt()])
    }
    #[inline(always)]
    unsafe fn sum(self) -> f32 {
        (self.0[0] + self.0[1]) + (self.0[2] + self.0[3])
    }
    #[inline(always)]
    unsafe fn any_inside(self, hi: Self) -> bool {
        (0..4).any(|i| self.0[i] > 0.0 && self.0[i] < hi.0[i])
    }
    #[inline(always)]
    unsafe fn max_where(self, o: Self, d: Self) -> Self {
        Plain(std::array::from_fn(|i| if d.0[i] > 0.0 { self.0[i].max(o.0[i]) } else { self.0[i] }))
    }
    #[inline(always)]
    unsafe fn top(self) -> f32 {
        self.0[0].max(self.0[1]).max(self.0[2].max(self.0[3]))
    }
    #[inline(always)]
    unsafe fn keep_le(self, limit: Self, value: Self) -> (Self, u32) {
        let over: [bool; 4] = std::array::from_fn(|i| self.0[i] > limit.0[i]);
        (Plain(std::array::from_fn(|i| if over[i] { 0.0 } else { value.0[i] })), (0..4).map(|i| (over[i] as u32) << i).sum())
    }
    #[inline(always)]
    unsafe fn keep_apart(self, limit: Self, near: Self, least: Self, value: Self) -> (Self, u32) {
        let out: [bool; 4] = std::array::from_fn(|i| self.0[i] > limit.0[i] || near.0[i] < least.0[i]);
        (Plain(std::array::from_fn(|i| if out[i] { 0.0 } else { value.0[i] })), (0..4).map(|i| (out[i] as u32) << i).sum())
    }
}

#[cfg(target_arch = "x86_64")]
pub mod x86 {
    use super::{refine, Simd};
    use std::arch::x86_64::*;

    #[derive(Clone, Copy)]
    pub struct Avx512(__m512);

    impl Simd for Avx512 {
        const LANES: usize = 16;
        #[inline(always)]
        unsafe fn splat(v: f32) -> Self {
            Avx512(_mm512_set1_ps(v))
        }
        #[inline(always)]
        unsafe fn load(p: *const f32) -> Self {
            Avx512(_mm512_loadu_ps(p))
        }
        #[inline(always)]
        unsafe fn add(self, o: Self) -> Self {
            Avx512(_mm512_add_ps(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn sub(self, o: Self) -> Self {
            Avx512(_mm512_sub_ps(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn mul(self, o: Self) -> Self {
            Avx512(_mm512_mul_ps(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn mul_add(self, a: Self, b: Self) -> Self {
            Avx512(_mm512_fmadd_ps(self.0, a.0, b.0))
        }
        #[inline(always)]
        unsafe fn neg_mul_add(self, a: Self, b: Self) -> Self {
            Avx512(_mm512_fnmadd_ps(self.0, a.0, b.0))
        }
        #[inline(always)]
        unsafe fn rsqrt(self) -> Self {
            // 14 bits straight from the instruction: enough without refinement.
            Avx512(_mm512_rsqrt14_ps(self.0))
        }
        #[inline(always)]
        unsafe fn sum(self) -> f32 {
            _mm512_reduce_add_ps(self.0)
        }
        #[inline(always)]
        unsafe fn any_inside(self, hi: Self) -> bool {
            let below = _mm512_cmp_ps_mask(self.0, hi.0, _CMP_LT_OQ);
            _mm512_mask_cmp_ps_mask(below, self.0, _mm512_setzero_ps(), _CMP_GT_OQ) != 0
        }
        #[inline(always)]
        unsafe fn max_where(self, o: Self, d: Self) -> Self {
            let real = _mm512_cmp_ps_mask(d.0, _mm512_setzero_ps(), _CMP_GT_OQ);
            Avx512(_mm512_mask_max_ps(self.0, real, self.0, o.0))
        }
        #[inline(always)]
        unsafe fn top(self) -> f32 {
            _mm512_reduce_max_ps(self.0)
        }
        #[inline(always)]
        unsafe fn keep_le(self, limit: Self, value: Self) -> (Self, u32) {
            let over = _mm512_cmp_ps_mask(self.0, limit.0, _CMP_GT_OQ);
            (Avx512(_mm512_maskz_mov_ps(!over, value.0)), over as u32)
        }
        #[inline(always)]
        unsafe fn keep_apart(self, limit: Self, near: Self, least: Self, value: Self) -> (Self, u32) {
            let out = _mm512_cmp_ps_mask(self.0, limit.0, _CMP_GT_OQ) | _mm512_cmp_ps_mask(near.0, least.0, _CMP_LT_OQ);
            (Avx512(_mm512_maskz_mov_ps(!out, value.0)), out as u32)
        }
    }

    #[derive(Clone, Copy)]
    pub struct Avx2(__m256);

    impl Simd for Avx2 {
        const LANES: usize = 8;
        #[inline(always)]
        unsafe fn splat(v: f32) -> Self {
            Avx2(_mm256_set1_ps(v))
        }
        #[inline(always)]
        unsafe fn load(p: *const f32) -> Self {
            Avx2(_mm256_loadu_ps(p))
        }
        #[inline(always)]
        unsafe fn add(self, o: Self) -> Self {
            Avx2(_mm256_add_ps(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn sub(self, o: Self) -> Self {
            Avx2(_mm256_sub_ps(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn mul(self, o: Self) -> Self {
            Avx2(_mm256_mul_ps(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn mul_add(self, a: Self, b: Self) -> Self {
            Avx2(_mm256_fmadd_ps(self.0, a.0, b.0))
        }
        #[inline(always)]
        unsafe fn neg_mul_add(self, a: Self, b: Self) -> Self {
            Avx2(_mm256_fnmadd_ps(self.0, a.0, b.0))
        }
        #[inline(always)]
        unsafe fn rsqrt(self) -> Self {
            // The instruction gives 12 bits; one Newton step doubles that.
            refine(self, Avx2(_mm256_rsqrt_ps(self.0)))
        }
        #[inline(always)]
        unsafe fn sum(self) -> f32 {
            let q = _mm_add_ps(_mm256_castps256_ps128(self.0), _mm256_extractf128_ps(self.0, 1));
            let d = _mm_add_ps(q, _mm_movehl_ps(q, q));
            _mm_cvtss_f32(_mm_add_ss(d, _mm_shuffle_ps(d, d, 1)))
        }
        #[inline(always)]
        unsafe fn any_inside(self, hi: Self) -> bool {
            let below = _mm256_cmp_ps(self.0, hi.0, _CMP_LT_OQ);
            let above = _mm256_cmp_ps(self.0, _mm256_setzero_ps(), _CMP_GT_OQ);
            _mm256_movemask_ps(_mm256_and_ps(below, above)) != 0
        }
        #[inline(always)]
        unsafe fn max_where(self, o: Self, d: Self) -> Self {
            // Zero where `d` is not positive, which no maximum of non-negative numbers minds.
            let real = _mm256_cmp_ps(d.0, _mm256_setzero_ps(), _CMP_GT_OQ);
            Avx2(_mm256_max_ps(self.0, _mm256_and_ps(o.0, real)))
        }
        #[inline(always)]
        unsafe fn top(self) -> f32 {
            let q = _mm_max_ps(_mm256_castps256_ps128(self.0), _mm256_extractf128_ps(self.0, 1));
            let d = _mm_max_ps(q, _mm_movehl_ps(q, q));
            _mm_cvtss_f32(_mm_max_ss(d, _mm_shuffle_ps(d, d, 1)))
        }
        #[inline(always)]
        unsafe fn keep_le(self, limit: Self, value: Self) -> (Self, u32) {
            let over = _mm256_cmp_ps(self.0, limit.0, _CMP_GT_OQ);
            (Avx2(_mm256_andnot_ps(over, value.0)), _mm256_movemask_ps(over) as u32)
        }
        #[inline(always)]
        unsafe fn keep_apart(self, limit: Self, near: Self, least: Self, value: Self) -> (Self, u32) {
            let out = _mm256_or_ps(_mm256_cmp_ps(self.0, limit.0, _CMP_GT_OQ), _mm256_cmp_ps(near.0, least.0, _CMP_LT_OQ));
            (Avx2(_mm256_andnot_ps(out, value.0)), _mm256_movemask_ps(out) as u32)
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub mod arm {
    use super::Simd;
    use std::arch::aarch64::*;

    #[derive(Clone, Copy)]
    pub struct Neon(float32x4_t);

    impl Simd for Neon {
        const LANES: usize = 4;
        #[inline(always)]
        unsafe fn splat(v: f32) -> Self {
            Neon(vdupq_n_f32(v))
        }
        #[inline(always)]
        unsafe fn load(p: *const f32) -> Self {
            Neon(vld1q_f32(p))
        }
        #[inline(always)]
        unsafe fn add(self, o: Self) -> Self {
            Neon(vaddq_f32(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn sub(self, o: Self) -> Self {
            Neon(vsubq_f32(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn mul(self, o: Self) -> Self {
            Neon(vmulq_f32(self.0, o.0))
        }
        #[inline(always)]
        unsafe fn mul_add(self, a: Self, b: Self) -> Self {
            Neon(vfmaq_f32(b.0, self.0, a.0))
        }
        #[inline(always)]
        unsafe fn neg_mul_add(self, a: Self, b: Self) -> Self {
            Neon(vfmsq_f32(b.0, self.0, a.0))
        }
        #[inline(always)]
        unsafe fn rsqrt(self) -> Self {
            // The estimate is good to 8 bits; one Newton step (which has its own instruction)
            // doubles that, on a par with the other instruction sets.
            let y = vrsqrteq_f32(self.0);
            Neon(vmulq_f32(y, vrsqrtsq_f32(vmulq_f32(self.0, y), y)))
        }
        #[inline(always)]
        unsafe fn sum(self) -> f32 {
            vaddvq_f32(self.0)
        }
        #[inline(always)]
        unsafe fn any_inside(self, hi: Self) -> bool {
            let inside = vandq_u32(vcltq_f32(self.0, hi.0), vcgtq_f32(self.0, vdupq_n_f32(0.0)));
            vmaxvq_u32(inside) != 0
        }
        #[inline(always)]
        unsafe fn max_where(self, o: Self, d: Self) -> Self {
            // Zero where `d` is not positive, which no maximum of non-negative numbers minds.
            let real = vcgtq_f32(d.0, vdupq_n_f32(0.0));
            Neon(vmaxq_f32(self.0, vreinterpretq_f32_u32(vandq_u32(vreinterpretq_u32_f32(o.0), real))))
        }
        #[inline(always)]
        unsafe fn top(self) -> f32 {
            vmaxvq_f32(self.0)
        }
        #[inline(always)]
        unsafe fn keep_le(self, limit: Self, value: Self) -> (Self, u32) {
            let over = vcgtq_f32(self.0, limit.0);
            let lanes = [1u32, 2, 4, 8];
            let bits = vaddvq_u32(vandq_u32(over, vld1q_u32(lanes.as_ptr())));
            (Neon(vreinterpretq_f32_u32(vbicq_u32(vreinterpretq_u32_f32(value.0), over))), bits)
        }
        #[inline(always)]
        unsafe fn keep_apart(self, limit: Self, near: Self, least: Self, value: Self) -> (Self, u32) {
            let out = vorrq_u32(vcgtq_f32(self.0, limit.0), vcltq_f32(near.0, least.0));
            let lanes = [1u32, 2, 4, 8];
            let bits = vaddvq_u32(vandq_u32(out, vld1q_u32(lanes.as_ptr())));
            (Neon(vreinterpretq_f32_u32(vbicq_u32(vreinterpretq_u32_f32(value.0), out))), bits)
        }
    }
}
