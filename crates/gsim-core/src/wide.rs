//! The hot loops take eight bodies at a time, each of the eight on a lane of its own with
//! sums of its own, in vector instructions where the processor has them. Every lane does
//! the same IEEE operations (`+ - * /`, `sqrt`, comparisons) in the same order whatever the
//! instructions, and the same code does one body by itself where there are fewer than
//! eight: so the bits that come out do not depend on the processor, only how long it takes.

use std::ops::{Add, Div, Mul, Sub};

/// Bodies taken together in the hot loops.
pub const LANES: usize = 8;

/// The eight lanes' sums as one, always added up in this order.
#[inline(always)]
pub fn total(s: &[f64; LANES]) -> f64 {
    ((s[0] + s[1]) + (s[2] + s[3])) + ((s[4] + s[5]) + (s[6] + s[7]))
}

/// Numbers side by side: one (`f64`) or eight (of whatever kind the processor has). The hot loops are written once,
/// over this.
pub trait Wide: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> {
    /// The outcome of a comparison, lane by lane.
    type Mask: Copy;
    fn splat(v: f64) -> Self;
    /// The first numbers of a list.
    fn load(from: &[f64]) -> Self;
    fn store(self, to: &mut [f64]);
    /// 1 where a flag is set and 0 where it is not.
    fn flags(from: &[bool]) -> Self;
    fn sqrt(self) -> Self;
    fn abs(self) -> Self;
    fn le(self, other: Self) -> Self::Mask;
    fn gt(self, other: Self) -> Self::Mask;
    fn and(a: Self::Mask, b: Self::Mask) -> Self::Mask;
    fn or(a: Self::Mask, b: Self::Mask) -> Self::Mask;
    fn not(a: Self::Mask) -> Self::Mask;
    fn any(a: Self::Mask) -> bool;
    /// `a` where the mask is set, `b` elsewhere.
    fn pick(mask: Self::Mask, a: Self, b: Self) -> Self;
}

impl Wide for f64 {
    type Mask = bool;

    #[inline(always)]
    fn splat(v: f64) -> Self {
        v
    }

    #[inline(always)]
    fn load(from: &[f64]) -> Self {
        from[0]
    }

    #[inline(always)]
    fn store(self, to: &mut [f64]) {
        to[0] = self;
    }

    #[inline(always)]
    fn flags(from: &[bool]) -> Self {
        from[0] as u8 as f64
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        f64::sqrt(self)
    }

    #[inline(always)]
    fn abs(self) -> Self {
        f64::abs(self)
    }

    #[inline(always)]
    fn le(self, other: Self) -> bool {
        self <= other
    }

    #[inline(always)]
    fn gt(self, other: Self) -> bool {
        self > other
    }

    #[inline(always)]
    fn and(a: bool, b: bool) -> bool {
        a & b
    }

    #[inline(always)]
    fn or(a: bool, b: bool) -> bool {
        a | b
    }

    #[inline(always)]
    fn not(a: bool) -> bool {
        !a
    }

    #[inline(always)]
    fn any(a: bool) -> bool {
        a
    }

    #[inline(always)]
    fn pick(mask: bool, a: Self, b: Self) -> Self {
        if mask { a } else { b }
    }
}

/// Eight numbers as plain arrays, each operation done lane by lane: for processors there
/// is nothing written out for below, and for checking those against.
#[derive(Clone, Copy)]
pub struct Plain([f64; LANES]);

macro_rules! each {
    ($trait:ident, $name:ident, $op:tt) => {
        impl $trait for Plain {
            type Output = Self;

            #[inline(always)]
            fn $name(self, other: Self) -> Self {
                Self(std::array::from_fn(|k| self.0[k] $op other.0[k]))
            }
        }
    };
}
each!(Add, add, +);
each!(Sub, sub, -);
each!(Mul, mul, *);
each!(Div, div, /);

impl Wide for Plain {
    /// All ones in a lane for yes, all zeros for no.
    type Mask = [u64; LANES];

    #[inline(always)]
    fn splat(v: f64) -> Self {
        Self([v; LANES])
    }

    #[inline(always)]
    fn load(from: &[f64]) -> Self {
        Self(from[..LANES].try_into().unwrap())
    }

    #[inline(always)]
    fn store(self, to: &mut [f64]) {
        to[..LANES].copy_from_slice(&self.0);
    }

    #[inline(always)]
    fn flags(from: &[bool]) -> Self {
        let from: [bool; LANES] = from[..LANES].try_into().unwrap();
        Self(from.map(|flag| flag as u8 as f64))
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        Self(self.0.map(f64::sqrt))
    }

    #[inline(always)]
    fn abs(self) -> Self {
        Self(self.0.map(f64::abs))
    }

    #[inline(always)]
    fn le(self, other: Self) -> Self::Mask {
        std::array::from_fn(|k| if self.0[k] <= other.0[k] { u64::MAX } else { 0 })
    }

    #[inline(always)]
    fn gt(self, other: Self) -> Self::Mask {
        std::array::from_fn(|k| if self.0[k] > other.0[k] { u64::MAX } else { 0 })
    }

    #[inline(always)]
    fn and(a: Self::Mask, b: Self::Mask) -> Self::Mask {
        std::array::from_fn(|k| a[k] & b[k])
    }

    #[inline(always)]
    fn or(a: Self::Mask, b: Self::Mask) -> Self::Mask {
        std::array::from_fn(|k| a[k] | b[k])
    }

    #[inline(always)]
    fn not(a: Self::Mask) -> Self::Mask {
        a.map(|lane| !lane)
    }

    #[inline(always)]
    fn any(a: Self::Mask) -> bool {
        a.iter().fold(0, |any, lane| any | lane) != 0
    }

    #[inline(always)]
    fn pick(mask: Self::Mask, a: Self, b: Self) -> Self {
        Self(std::array::from_fn(|k| f64::from_bits((a.0[k].to_bits() & mask[k]) | (b.0[k].to_bits() & !mask[k]))))
    }
}

/// Eight numbers in the registers of an x86 processor: four of two (every one has those),
/// two of four (AVX2) or one of eight (AVX-512).
#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::{Wide, LANES};
    use std::arch::x86_64::*;
    use std::ops::{Add, Div, Mul, Sub};

    // SAFETY (for all of the below): each kind is only used where the processor has its
    // instructions (see `widest`), and loads and stores are within slices checked to hold
    // eight.
    macro_rules! each {
        ($kind:ident, $trait:ident, $name:ident, $op:ident) => {
            impl $trait for $kind {
                type Output = Self;

                #[inline(always)]
                fn $name(self, other: Self) -> Self {
                    Self(std::array::from_fn(|k| unsafe { $op(self.0[k], other.0[k]) }))
                }
            }
        };
    }

    /// The kinds whose comparisons give a register of all ones or all zeros in each lane.
    macro_rules! registers {
        ($kind:ident, $reg:ident, $count:literal, $per:literal, $set1:ident, $loadu:ident, $storeu:ident, $add:ident, $sub:ident, $mul:ident, $div:ident,
         $sqrt:ident, $and:ident, $or:ident, $xor:ident, $andnot:ident, $movemask:ident, $le:expr, $gt:expr) => {
            #[derive(Clone, Copy)]
            pub struct $kind([$reg; $count]);

            each!($kind, Add, add, $add);
            each!($kind, Sub, sub, $sub);
            each!($kind, Mul, mul, $mul);
            each!($kind, Div, div, $div);

            impl Wide for $kind {
                type Mask = [$reg; $count];

                #[inline(always)]
                fn splat(v: f64) -> Self {
                    Self([unsafe { $set1(v) }; $count])
                }

                #[inline(always)]
                fn load(from: &[f64]) -> Self {
                    let from = &from[..LANES];
                    Self(std::array::from_fn(|k| unsafe { $loadu(from.as_ptr().add($per * k)) }))
                }

                #[inline(always)]
                fn store(self, to: &mut [f64]) {
                    let to = &mut to[..LANES];
                    for k in 0..$count {
                        unsafe { $storeu(to.as_mut_ptr().add($per * k), self.0[k]) };
                    }
                }

                #[inline(always)]
                fn flags(from: &[bool]) -> Self {
                    let from: [bool; LANES] = from[..LANES].try_into().unwrap();
                    Self::load(&from.map(|flag| flag as u8 as f64))
                }

                #[inline(always)]
                fn sqrt(self) -> Self {
                    Self(self.0.map(|v| unsafe { $sqrt(v) }))
                }

                #[inline(always)]
                fn abs(self) -> Self {
                    // (Everything but the sign.)
                    Self(self.0.map(|v| unsafe { $andnot($set1(-0.0), v) }))
                }

                #[inline(always)]
                fn le(self, other: Self) -> Self::Mask {
                    std::array::from_fn(|k| unsafe { $le(self.0[k], other.0[k]) })
                }

                #[inline(always)]
                fn gt(self, other: Self) -> Self::Mask {
                    std::array::from_fn(|k| unsafe { $gt(self.0[k], other.0[k]) })
                }

                #[inline(always)]
                fn and(a: Self::Mask, b: Self::Mask) -> Self::Mask {
                    std::array::from_fn(|k| unsafe { $and(a[k], b[k]) })
                }

                #[inline(always)]
                fn or(a: Self::Mask, b: Self::Mask) -> Self::Mask {
                    std::array::from_fn(|k| unsafe { $or(a[k], b[k]) })
                }

                #[inline(always)]
                fn not(a: Self::Mask) -> Self::Mask {
                    a.map(|m| unsafe { $xor(m, $set1(f64::from_bits(u64::MAX))) })
                }

                #[inline(always)]
                fn any(a: Self::Mask) -> bool {
                    a.iter().fold(0, |any, m| any | unsafe { $movemask(*m) }) != 0
                }

                #[inline(always)]
                fn pick(mask: Self::Mask, a: Self, b: Self) -> Self {
                    Self(std::array::from_fn(|k| unsafe { $or($and(mask[k], a.0[k]), $andnot(mask[k], b.0[k])) }))
                }
            }
        };
    }

    registers!(
        Sse2, __m128d, 4, 2, _mm_set1_pd, _mm_loadu_pd, _mm_storeu_pd, _mm_add_pd, _mm_sub_pd, _mm_mul_pd, _mm_div_pd,
        _mm_sqrt_pd, _mm_and_pd, _mm_or_pd, _mm_xor_pd, _mm_andnot_pd, _mm_movemask_pd, _mm_cmple_pd, _mm_cmpgt_pd
    );
    registers!(
        Avx2, __m256d, 2, 4, _mm256_set1_pd, _mm256_loadu_pd, _mm256_storeu_pd, _mm256_add_pd, _mm256_sub_pd, _mm256_mul_pd, _mm256_div_pd,
        _mm256_sqrt_pd, _mm256_and_pd, _mm256_or_pd, _mm256_xor_pd, _mm256_andnot_pd, _mm256_movemask_pd,
        |a, b| _mm256_cmp_pd::<_CMP_LE_OQ>(a, b), |a, b| _mm256_cmp_pd::<_CMP_GT_OQ>(a, b)
    );

    /// All eight in one register; its comparisons give a bit for each lane.
    #[derive(Clone, Copy)]
    pub struct Avx512(__m512d);

    macro_rules! whole {
        ($trait:ident, $name:ident, $op:ident) => {
            impl $trait for Avx512 {
                type Output = Self;

                #[inline(always)]
                fn $name(self, other: Self) -> Self {
                    Self(unsafe { $op(self.0, other.0) })
                }
            }
        };
    }
    whole!(Add, add, _mm512_add_pd);
    whole!(Sub, sub, _mm512_sub_pd);
    whole!(Mul, mul, _mm512_mul_pd);
    whole!(Div, div, _mm512_div_pd);

    impl Wide for Avx512 {
        type Mask = __mmask8;

        #[inline(always)]
        fn splat(v: f64) -> Self {
            Self(unsafe { _mm512_set1_pd(v) })
        }

        #[inline(always)]
        fn load(from: &[f64]) -> Self {
            Self(unsafe { _mm512_loadu_pd(from[..LANES].as_ptr()) })
        }

        #[inline(always)]
        fn store(self, to: &mut [f64]) {
            unsafe { _mm512_storeu_pd(to[..LANES].as_mut_ptr(), self.0) };
        }

        #[inline(always)]
        fn flags(from: &[bool]) -> Self {
            let from: [bool; LANES] = from[..LANES].try_into().unwrap();
            Self::load(&from.map(|flag| flag as u8 as f64))
        }

        #[inline(always)]
        fn sqrt(self) -> Self {
            Self(unsafe { _mm512_sqrt_pd(self.0) })
        }

        #[inline(always)]
        fn abs(self) -> Self {
            Self(unsafe { _mm512_abs_pd(self.0) })
        }

        #[inline(always)]
        fn le(self, other: Self) -> __mmask8 {
            unsafe { _mm512_cmp_pd_mask::<_CMP_LE_OQ>(self.0, other.0) }
        }

        #[inline(always)]
        fn gt(self, other: Self) -> __mmask8 {
            unsafe { _mm512_cmp_pd_mask::<_CMP_GT_OQ>(self.0, other.0) }
        }

        #[inline(always)]
        fn and(a: __mmask8, b: __mmask8) -> __mmask8 {
            a & b
        }

        #[inline(always)]
        fn or(a: __mmask8, b: __mmask8) -> __mmask8 {
            a | b
        }

        #[inline(always)]
        fn not(a: __mmask8) -> __mmask8 {
            !a
        }

        #[inline(always)]
        fn any(a: __mmask8) -> bool {
            a != 0
        }

        #[inline(always)]
        fn pick(mask: __mmask8, a: Self, b: Self) -> Self {
            Self(unsafe { _mm512_mask_blend_pd(mask, b.0, a.0) })
        }
    }
}
#[cfg(target_arch = "x86_64")]
pub use x86::{Avx2, Avx512, Sse2};

/// Eight numbers as four NEON registers of two. (Written out by hand: left to itself, the
/// compiler does these loops one number at a time on this kind of processor.) Multiplying
/// and adding are always two instructions, never the fused one, which rounds differently.
#[cfg(target_arch = "aarch64")]
mod arm {
    use super::{Wide, LANES};
    use std::arch::aarch64::*;
    use std::ops::{Add, Div, Mul, Sub};

    #[derive(Clone, Copy)]
    pub struct Neon([float64x2_t; 4]);

    // SAFETY (for all of the below): every processor of this kind has NEON, and loads and
    // stores are within slices checked to hold eight.
    macro_rules! each {
        ($trait:ident, $name:ident, $op:ident) => {
            impl $trait for Neon {
                type Output = Self;

                #[inline(always)]
                fn $name(self, other: Self) -> Self {
                    Self(std::array::from_fn(|k| unsafe { $op(self.0[k], other.0[k]) }))
                }
            }
        };
    }
    each!(Add, add, vaddq_f64);
    each!(Sub, sub, vsubq_f64);
    each!(Mul, mul, vmulq_f64);
    each!(Div, div, vdivq_f64);

    impl Wide for Neon {
        type Mask = [uint64x2_t; 4];

        #[inline(always)]
        fn splat(v: f64) -> Self {
            Self([unsafe { vdupq_n_f64(v) }; 4])
        }

        #[inline(always)]
        fn load(from: &[f64]) -> Self {
            let from = &from[..LANES];
            Self(std::array::from_fn(|k| unsafe { vld1q_f64(from.as_ptr().add(2 * k)) }))
        }

        #[inline(always)]
        fn store(self, to: &mut [f64]) {
            let to = &mut to[..LANES];
            for k in 0..4 {
                unsafe { vst1q_f64(to.as_mut_ptr().add(2 * k), self.0[k]) };
            }
        }

        #[inline(always)]
        fn flags(from: &[bool]) -> Self {
            let from: [bool; LANES] = from[..LANES].try_into().unwrap();
            Self::load(&from.map(|flag| flag as u8 as f64))
        }

        #[inline(always)]
        fn sqrt(self) -> Self {
            Self(self.0.map(|v| unsafe { vsqrtq_f64(v) }))
        }

        #[inline(always)]
        fn abs(self) -> Self {
            Self(self.0.map(|v| unsafe { vabsq_f64(v) }))
        }

        #[inline(always)]
        fn le(self, other: Self) -> Self::Mask {
            std::array::from_fn(|k| unsafe { vcleq_f64(self.0[k], other.0[k]) })
        }

        #[inline(always)]
        fn gt(self, other: Self) -> Self::Mask {
            std::array::from_fn(|k| unsafe { vcgtq_f64(self.0[k], other.0[k]) })
        }

        #[inline(always)]
        fn and(a: Self::Mask, b: Self::Mask) -> Self::Mask {
            std::array::from_fn(|k| unsafe { vandq_u64(a[k], b[k]) })
        }

        #[inline(always)]
        fn or(a: Self::Mask, b: Self::Mask) -> Self::Mask {
            std::array::from_fn(|k| unsafe { vorrq_u64(a[k], b[k]) })
        }

        #[inline(always)]
        fn not(a: Self::Mask) -> Self::Mask {
            a.map(|m| unsafe { veorq_u64(m, vdupq_n_u64(u64::MAX)) })
        }

        #[inline(always)]
        fn any(a: Self::Mask) -> bool {
            unsafe {
                let all = vorrq_u64(vorrq_u64(a[0], a[1]), vorrq_u64(a[2], a[3]));
                (vgetq_lane_u64::<0>(all) | vgetq_lane_u64::<1>(all)) != 0
            }
        }

        #[inline(always)]
        fn pick(mask: Self::Mask, a: Self, b: Self) -> Self {
            Self(std::array::from_fn(|k| unsafe { vbslq_f64(mask[k], a.0[k], b.0[k]) }))
        }
    }
}
#[cfg(target_arch = "aarch64")]
pub use arm::Neon;

/// What every processor of the kind compiled for has.
#[cfg(target_arch = "x86_64")]
pub type Base = Sse2;
#[cfg(target_arch = "aarch64")]
pub type Base = Neon;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub type Base = Plain;

/// The widest vectors (in bits) the hot loops may be run with: for tests, which check that
/// the width makes no difference.
static WIDEST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

#[doc(hidden)]
pub fn limit_vectors(bits: u32) {
    WIDEST.store(bits, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn widest_allowed() -> u32 {
    WIDEST.load(std::sync::atomic::Ordering::Relaxed)
}

/// A function that runs `$body`, which is generic over the kind of eight lanes, with the
/// widest kind this processor has. (`$body` has to be `#[inline(always)]`.)
macro_rules! widest {
    ($(#[$meta:meta])* fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)? = $body:ident) => {
        $(#[$meta])*
        fn $name($($arg: $ty),*) $(-> $ret)? {
            let allowed = $crate::wide::widest_allowed();
            #[cfg(target_arch = "x86_64")]
            {
                #[target_feature(enable = "avx512f")]
                unsafe fn wide512($($arg: $ty),*) $(-> $ret)? {
                    $body::<$crate::wide::Avx512>($($arg),*)
                }
                #[target_feature(enable = "avx2")]
                unsafe fn wide256($($arg: $ty),*) $(-> $ret)? {
                    $body::<$crate::wide::Avx2>($($arg),*)
                }
                // SAFETY: each is called only where the processor has what it was compiled for.
                if allowed >= 512 && std::arch::is_x86_feature_detected!("avx512f") {
                    return unsafe { wide512($($arg),*) };
                }
                if allowed >= 256 && std::arch::is_x86_feature_detected!("avx2") {
                    return unsafe { wide256($($arg),*) };
                }
            }
            if allowed >= 128 {
                $body::<$crate::wide::Base>($($arg),*)
            } else {
                $body::<$crate::wide::Plain>($($arg),*)
            }
        }
    };
}
pub(crate) use widest;

/// What the hot loops are run with on this machine, for showing.
pub fn kind() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if widest_allowed() >= 512 && std::arch::is_x86_feature_detected!("avx512f") {
            return "AVX-512";
        }
        if widest_allowed() >= 256 && std::arch::is_x86_feature_detected!("avx2") {
            return "AVX2";
        }
    }
    if cfg!(target_arch = "x86_64") {
        "SSE2"
    } else if cfg!(target_arch = "aarch64") {
        "NEON"
    } else {
        "plain arrays"
    }
}
