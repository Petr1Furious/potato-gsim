//! "Hold an orbit" objective: is a ship on a closed, reasonably round, close orbit around a body?

use crate::{GameRules, Particle};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OrbitStatus {
    /// Negative orbital energy relative to the body.
    pub bound: bool,
    pub ecc: f64,
    /// Closest and farthest distance from the body's centre along the orbit (m).
    pub peri: f64,
    pub apo: f64,
    pub ok: bool,
}

/// Two-body orbital elements of `ship` around a body (`body` = its position and velocity).
pub fn orbit_status(ship: &Particle, body: &Particle, mass: f64, radius: f64, rules: &GameRules) -> OrbitStatus {
    let mu = rules.g * mass;
    let (rx, ry) = (ship.x - body.x, ship.y - body.y);
    let (vx, vy) = (ship.vx - body.vx, ship.vy - body.vy);
    let r = (rx * rx + ry * ry).sqrt();
    if !(mu > 0.0 && r > 0.0) {
        return OrbitStatus::default();
    }
    let energy = 0.5 * (vx * vx + vy * vy) - mu / r;
    if energy >= 0.0 {
        return OrbitStatus::default();
    }
    let h = rx * vy - ry * vx;
    let a = -mu / (2.0 * energy);
    let ecc = (1.0 + 2.0 * energy * h * h / (mu * mu)).max(0.0).sqrt();
    let (peri, apo) = (a * (1.0 - ecc), a * (1.0 + ecc));
    let ok = ecc <= rules.orbit_max_ecc && peri >= rules.orbit_min_peri_radii * radius && apo <= rules.orbit_max_apo_radii * radius;
    OrbitStatus { bound: true, ecc, peri, apo, ok }
}
