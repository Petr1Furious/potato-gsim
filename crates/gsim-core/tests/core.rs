// Tests may use any float function: only the library has to be bit-reproducible.
#![allow(clippy::disallowed_methods)]

use gsim_core::particle::{step_particle, substeps};
use gsim_core::predict::predict_ship;
use gsim_core::ship::step_ship;
use gsim_core::*;
use std::sync::Arc;

fn rules() -> GameRules {
    GameRules::new(86400.0, 60)
}

fn body(x: f64, y: f64, vx: f64, vy: f64, mass: f64, radius: f64) -> Body {
    Body { x, y, vx, vy, mass, radius }
}

/// Deterministic pseudo-random cloud, dense enough that merges happen.
fn cloud(n: usize, negative_every: usize) -> Vec<Body> {
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..n)
        .map(|i| {
            let sign = if negative_every > 0 && i % negative_every == 0 { -1.0 } else { 1.0 };
            body(
                (next() - 0.5) * 4.0e10,
                (next() - 0.5) * 4.0e10,
                (next() - 0.5) * 2.0e3,
                (next() - 0.5) * 2.0e3,
                sign * (1.0e22 + next() * 5.0e25),
                5.0e6 + next() * 4.0e7,
            )
        })
        .collect()
}

fn run(bodies: &[Body], ticks: u32) -> MassiveState {
    let r = rules();
    let mut s = MassiveState::from_bodies(bodies);
    for _ in 0..ticks {
        s.step(&r);
    }
    s
}

#[test]
fn merge_keeps_heavier_and_conserves_mass_and_momentum() {
    let r = rules();
    let mut s = MassiveState::from_bodies(&[
        body(0.0, 0.0, 10.0, 0.0, 5.0e20, 2.0e6),
        body(1.0e6, 0.0, -4.0, 2.0, 9.0e20, 2.0e6),
        body(1.0e12, 0.0, 0.0, 0.0, 1.0e20, 1.0e6),
    ]);
    let (px0, py0) = s.momentum();
    let ev = s.step(&r);
    assert_eq!(ev, vec![MergeEvent { survivor: Some(1), absorbed: vec![0] }]);
    assert_eq!(s.alive, vec![false, true, true]);
    assert_eq!(s.mass[1], 14.0e20);
    let (px1, py1) = s.momentum();
    assert!((px1 - px0).abs() <= 1e-9 * px0.abs().max(1.0e20));
    assert!((py1 - py0).abs() <= 1e-9 * py0.abs().max(1.0e20));
    // Volumes add.
    let expect = (2.0f64 * 8.0e18).powf(1.0 / 3.0);
    assert!((s.radius[1] - expect).abs() / expect < 1e-12);
}

#[test]
fn merge_tie_keeps_lower_slot() {
    let r = rules();
    let mut s = MassiveState::from_bodies(&[
        body(0.0, 0.0, 0.0, 0.0, 8.0e20, 2.0e6),
        body(1.0e6, 0.0, 0.0, 0.0, 8.0e20, 2.0e6),
    ]);
    let ev = s.step(&r);
    assert_eq!(ev[0].survivor, Some(0));
    assert_eq!(s.mass[0], 16.0e20);
}

#[test]
fn opposite_masses_annihilate_or_cancel() {
    let r = rules();
    let mut s = MassiveState::from_bodies(&[
        body(0.0, 0.0, 0.0, 0.0, 8.0e20, 2.0e6),
        body(1.0e6, 0.0, 0.0, 0.0, -8.0e20, 2.0e6),
    ]);
    let ev = s.step(&r);
    assert_eq!(ev, vec![MergeEvent { survivor: None, absorbed: vec![0, 1] }]);
    assert_eq!(s.alive_count(), 0);

    let mut s = MassiveState::from_bodies(&[
        body(0.0, 0.0, 0.0, 0.0, -5.0e20, 2.0e6),
        body(1.0e6, 0.0, 0.0, 0.0, 9.0e20, 2.0e6),
    ]);
    let ev = s.step(&r);
    assert_eq!(ev[0].survivor, Some(1));
    assert_eq!(s.mass[1], 4.0e20);
    assert!(s.radius[1] > 0.0 && s.radius[1] < 2.0e6);
}

#[test]
fn negative_mass_repels() {
    let r = rules();
    let mut s = MassiveState::from_bodies(&[
        body(0.0, 0.0, 0.0, 0.0, -1.0e26, 1.0e6),
        body(1.0e9, 0.0, 0.0, 0.0, 1.0e20, 1.0e6),
    ]);
    for _ in 0..10 {
        s.step(&r);
    }
    assert!(s.x[1] > 1.0e9, "light body is pushed away");
    assert!(s.x[0] > 0.0, "negative body chases the positive one");
}

#[test]
fn hash_is_independent_of_thread_count() {
    let bodies = cloud(400, 9);
    let mut hashes = Vec::new();
    let mut alive = Vec::new();
    for threads in [1usize, 2, 7, 16] {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        let s = pool.install(|| run(&bodies, 150));
        hashes.push(s.hash());
        alive.push(s.alive_count());
    }
    assert!(alive[0] < 400, "scene should exercise merging");
    assert!(hashes.iter().all(|h| *h == hashes[0]), "{hashes:x?}");
}

#[test]
fn snapshot_restore_continues_identically() {
    let r = rules();
    let bodies = cloud(250, 0);
    let straight = run(&bodies, 120);
    let mut a = run(&bodies, 47);
    let snap = a.snapshot();
    let bytes_equal = MassiveState::from_snapshot(&snap).hash() == a.hash();
    assert!(bytes_equal);
    let mut b = MassiveState::from_snapshot(&snap);
    for _ in 47..120 {
        a.step(&r);
        b.step(&r);
    }
    assert_eq!(a.hash(), straight.hash());
    assert_eq!(b.hash(), straight.hash());
}

#[test]
fn ephemeris_rows_match_live_state() {
    let r = rules();
    let bodies = cloud(220, 0);
    let mut live = MassiveState::from_bodies(&bodies);
    let mut prod = EphProducer::new(&live.snapshot(), r.clone());
    let mut ring = EphRing::new();
    for _ in 0..80 {
        let h = live.hash();
        let row = prod.next_row();
        assert_eq!(row.hash, h);
        assert_eq!(row.tick, live.tick);
        ring.push(row);
        live.step(&r);
    }
    assert_eq!(ring.first_tick(), Some(0));
    assert_eq!(ring.end_tick(), Some(80));
    ring.trim_before(30);
    assert!(ring.get(29).is_none());
    assert_eq!(ring.get(30).unwrap().tick, 30);
}

#[test]
fn two_body_orbit_conserves_energy() {
    let r = rules();
    let m = 2.0e30;
    let d = 1.5e11;
    let v = (r.g * m / d).sqrt();
    let mut s = MassiveState::from_bodies(&[
        body(0.0, 0.0, 0.0, 0.0, m, 7.0e8),
        body(d, 0.0, 0.0, v, 6.0e24, 6.4e6),
    ]);
    let e0 = s.energy(&r);
    // Three simulated years.
    for _ in 0..(3.0 * 365.25 * 86400.0 / r.dt) as u32 {
        s.step(&r);
    }
    let e1 = s.energy(&r);
    assert!(((e1 - e0) / e0).abs() < 1e-6, "energy drift {}", (e1 - e0) / e0);
    let dist = ((s.x[1] - s.x[0]).powi(2) + (s.y[1] - s.y[0]).powi(2)).sqrt();
    assert!((dist - d).abs() / d < 1e-3);
}

fn planet_row(r: &GameRules) -> (MassiveState, f64) {
    let mut s = MassiveState::from_bodies(&[body(0.0, 0.0, 0.0, 0.0, 5.97e24, 6.371e6)]);
    s.ensure_acc(r);
    (s, 5.97e24)
}

#[test]
fn low_orbit_is_stable_thanks_to_substeps() {
    let r = rules();
    let (s, m) = planet_row(&r);
    let alt = 6.371e6 + 4.0e5;
    let v = (r.g * m / alt).sqrt();
    let mut p = Particle { x: alt, y: 0.0, vx: 0.0, vy: v };
    assert!(substeps(&p, &s.view(), &r) >= 16, "a 1440 s tick is far too coarse for low orbit");
    let mut scratch = Scratch::default();
    let (mut lo, mut hi) = (f64::MAX, 0.0f64);
    // ~80 orbits.
    for _ in 0..300 {
        assert_eq!(step_particle(&mut p, (0.0, 0.0), 15.0, &s.view(), &r, &mut scratch), None);
        let d = (p.x * p.x + p.y * p.y).sqrt();
        lo = lo.min(d);
        hi = hi.max(d);
    }
    assert!(lo > alt * 0.99 && hi < alt * 1.01, "orbit radius wandered: {lo} .. {hi}");
}

#[test]
fn fast_particle_cannot_tunnel_through_a_body() {
    let r = rules();
    let (s, _) = planet_row(&r);
    // 30 km/s covers 4.3e7 m per tick: starts well outside, would end far on the other side.
    let mut p = Particle { x: -2.0e7, y: 1.0e6, vx: 3.0e4, vy: 0.0 };
    let hit = step_particle(&mut p, (0.0, 0.0), 15.0, &s.view(), &r, &mut Scratch::default());
    assert_eq!(hit, Some(0));
    // And a clean miss is not reported.
    let mut p = Particle { x: -2.0e7, y: 5.0e7, vx: 3.0e4, vy: 0.0 };
    let hit = step_particle(&mut p, (0.0, 0.0), 15.0, &s.view(), &r, &mut Scratch::default());
    assert_eq!(hit, None);
}

#[test]
fn fuel_burns_and_regenerates_in_integers() {
    let r = rules();
    let (s, _) = planet_row(&r);
    let mut ship = ShipState::new(Particle { x: 1.0e10, y: 0.0, vx: 0.0, vy: 0.0 }, &r);
    let mut scratch = Scratch::default();
    let full = ShipInput { angle: 0, thrust: 100 };
    step_ship(&mut ship, full, &s.view(), &r, &mut scratch);
    assert_eq!(ship.fuel, r.fuel_max_mmps - r.burn_per_tick_mmps);
    let dv = r.burn_per_tick_mmps as f64 / 1000.0;
    assert!((ship.p.vx - dv).abs() < 0.05 * dv, "vx {} vs {}", ship.p.vx, dv);
    // Burn dry: never negative, thrust stops.
    while ship.fuel > 0 {
        step_ship(&mut ship, full, &s.view(), &r, &mut scratch);
    }
    assert_eq!(ship.fuel, 0);
    let idle = ShipInput::default();
    while ship.idle_ticks < r.regen_delay_ticks - 1 {
        step_ship(&mut ship, idle, &s.view(), &r, &mut scratch);
        assert_eq!(ship.fuel, 0);
    }
    step_ship(&mut ship, idle, &s.view(), &r, &mut scratch);
    assert_eq!(ship.fuel, r.regen_per_tick_mmps);
}

#[test]
fn timeline_semantics() {
    let mut t = InputTimeline::new();
    assert_eq!(t.at(5), ShipInput::default());
    let a = ShipInput { angle: 100, thrust: 50 };
    let b = ShipInput { angle: 200, thrust: 0 };
    t.set(10, a);
    t.set(20, b);
    assert_eq!(t.at(9), ShipInput::default());
    assert_eq!(t.at(10), a);
    assert_eq!(t.at(19), a);
    assert_eq!(t.at(20), b);
    t.prune_before(15);
    assert_eq!(t.at(15), a);
    assert_eq!(t.at(25), b);
    assert_eq!(t.changes().len(), 2);
}

#[test]
fn prediction_is_bit_identical_to_what_then_happens() {
    let r = rules();
    let bodies = cloud(60, 7);
    let mut prod = EphProducer::new(&MassiveState::from_bodies(&bodies).snapshot(), r.clone());
    let mut ring = EphRing::new();
    for _ in 0..200 {
        ring.push(prod.next_row());
    }
    let mut timeline = InputTimeline::new();
    timeline.set(3, ShipInput { angle: 12000, thrust: 100 });
    timeline.set(40, ShipInput { angle: 40000, thrust: 35 });
    timeline.set(90, ShipInput { angle: 40000, thrust: 0 });
    let start = ShipState::new(Particle { x: 3.0e10, y: -1.0e10, vx: -500.0, vy: 900.0 }, &r);
    let path = predict_ship(start, 0, 200, |t, _| timeline.at(t), |t| ring.get(t), &r);

    let mut ship = start;
    let mut scratch = Scratch::default();
    let mut flown = vec![(ship.p.x, ship.p.y)];
    for t in 0..200u64 {
        let row: Arc<EphRow> = ring.get(t).unwrap();
        let hit = step_ship(&mut ship, timeline.at(t), &row.view(), &r, &mut scratch);
        flown.push((ship.p.x, ship.p.y));
        if hit.is_some() {
            break;
        }
    }
    assert_eq!(path.points.len(), flown.len());
    for (a, b) in path.points.iter().zip(&flown) {
        assert_eq!(a.0.to_bits(), b.0.to_bits());
        assert_eq!(a.1.to_bits(), b.1.to_bits());
    }
}

#[test]
fn golden_selftest() {
    let h = selftest::compute();
    println!("golden hash: {h:#018x}");
    assert_eq!(h, selftest::compute(), "self-test must be repeatable");
    assert_eq!(h, selftest::GOLDEN, "simulation changed: update selftest::GOLDEN deliberately");
}

#[test]
fn escaped_bodies_are_removed_but_bound_ones_stay() {
    let mut r = rules();
    r.escape_radius = 1.0e12;
    let star = 2.0e30;
    let far = 2.0e12;
    let v_circ = (r.g * star / far).sqrt();
    let mut s = MassiveState::from_bodies(&[
        body(0.0, 0.0, 0.0, 0.0, star, 7.0e8),
        // Beyond the radius but on a bound circular orbit: stays.
        body(far, 0.0, 0.0, v_circ, 1.0e24, 6.0e6),
        // Beyond the radius, faster than escape speed, heading out: removed.
        body(0.0, far, 0.0, 3.0 * v_circ, 1.0e24, 6.0e6),
        // Just as fast but heading inwards: not yet.
        body(-far, 0.0, 3.0 * v_circ, 0.0, 1.0e24, 6.0e6),
        // Fast and outgoing but still inside the radius: not yet.
        body(0.0, -5.0e11, 0.0, -9.0 * v_circ, 1.0e24, 6.0e6),
    ]);
    let mut removed = Vec::new();
    for _ in 0..r.escape_check_ticks {
        for e in s.step(&r) {
            assert_eq!(e.survivor, None);
            removed.extend(e.absorbed);
        }
    }
    assert_eq!(removed, vec![2]);
    assert_eq!(s.alive, vec![true, true, false, true, true]);
    // The rule is part of the deterministic state: same result when replayed from a snapshot.
    let mut again = MassiveState::from_snapshot(&MassiveState::from_bodies(&[body(0.0, 0.0, 0.0, 0.0, star, 7.0e8), body(0.0, far, 0.0, 3.0 * v_circ, 1.0e24, 6.0e6)]).snapshot());
    for _ in 0..r.escape_check_ticks {
        again.step(&r);
    }
    assert_eq!(again.alive_count(), 1);
}
