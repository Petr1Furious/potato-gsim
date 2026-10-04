//! A fixed scene whose final hash is compiled into every binary. If a machine computes a
//! different value, its floating point does not match everyone else's and it must not join.

use crate::massive::{Body, MassiveState};
use crate::particle::Scratch;
use crate::ship::{step_ship, ShipInput, ShipState};
use crate::{GameRules, Particle};
use xxhash_rust::xxh3::Xxh3;

/// Expected result of [`compute`]. Regenerate with `cargo test -p gsim-core -- --nocapture golden`
/// only when the simulation is intentionally changed (and bump the protocol's sim version).
pub const GOLDEN: u64 = 0x5ae5_b56e_ddf5_f97f;

pub fn scene() -> Vec<Body> {
    // Integer LCG and exact arithmetic only: the scene itself must be reproducible.
    let mut s: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut bodies = vec![Body { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, mass: 2.0e30, radius: 7.0e8 }];
    for i in 0..15 {
        let r = 4.0e10 + 3.0e10 * i as f64;
        let v = (6.67430e-11 * 2.0e30 / r).sqrt();
        let (x, y, vx, vy) = match i % 4 {
            0 => (r, 0.0, 0.0, v),
            1 => (0.0, r, -v, 0.0),
            2 => (-r, 0.0, 0.0, -v),
            _ => (0.0, -r, v, 0.0),
        };
        let mass = (1.0e23 + next() * 5.0e26) * if i == 7 { -1.0 } else { 1.0 };
        bodies.push(Body { x, y, vx, vy, mass, radius: 2.0e6 + next() * 6.0e7 });
    }
    bodies
}

pub fn compute() -> u64 {
    let rules = GameRules::new(86400.0, 60);
    let mut state = MassiveState::from_bodies(&scene());
    let mut ship = ShipState::new(Particle { x: 4.0e10, y: 4.0e8, vx: 0.0, vy: 6.0e4 }, &rules);
    let mut scratch = Scratch::default();
    for t in 0..1000u32 {
        state.ensure_acc(&rules);
        let input = ShipInput { angle: (t.wrapping_mul(2654435761) >> 16) as u16, thrust: (t % 101) as u8 };
        step_ship(&mut ship, input, &state.view(), &rules, &mut scratch);
        state.step(&rules);
    }
    let mut h = Xxh3::new();
    h.update(&state.hash().to_le_bytes());
    for v in [ship.p.x, ship.p.y, ship.p.vx, ship.p.vy] {
        h.update(&v.to_bits().to_le_bytes());
    }
    h.update(&ship.fuel.to_le_bytes());
    h.digest()
}

pub fn passes() -> bool {
    compute() == GOLDEN
}
