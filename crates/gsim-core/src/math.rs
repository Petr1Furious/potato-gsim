//! The only place transcendental functions may be used. All of them are pure-Rust libm,
//! so results do not depend on the platform's C library.

pub fn cbrt(x: f64) -> f64 {
    libm::cbrt(x)
}

pub const TAU: f64 = std::f64::consts::TAU;

/// Unit vector for a quantised heading (`0..65536` maps to a full turn).
pub fn angle_to_dir(angle: u16) -> (f64, f64) {
    let a = angle as f64 * (TAU / 65536.0);
    (libm::cos(a), libm::sin(a))
}

pub fn angle_to_radians(angle: u16) -> f64 {
    angle as f64 * (TAU / 65536.0)
}

/// Quantise a direction. Only used where the result is transmitted, never re-derived.
pub fn dir_to_angle(x: f64, y: f64) -> u16 {
    let a = libm::atan2(y, x);
    let turns = a / TAU;
    let q = libm::round(turns * 65536.0) as i64;
    q.rem_euclid(65536) as u16
}
