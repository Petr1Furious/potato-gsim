use gsim_core::{GameRules, MassiveState};
use gsim_server::scenario::{build, RandomOpts, PRESETS};

fn rules() -> GameRules {
    GameRules::new(86400.0, 60)
}

fn dist(s: &MassiveState, a: usize, b: usize) -> f64 {
    ((s.x[a] - s.x[b]).powi(2) + (s.y[a] - s.y[b]).powi(2)).sqrt()
}

#[test]
fn every_preset_builds_and_runs() {
    let r = rules();
    for (name, _) in PRESETS {
        let sc = build(name, 7, &RandomOpts { count: 300, ..Default::default() }).unwrap();
        assert!(!sc.bodies.is_empty(), "{name}");
        assert!(sc.spawn_r.0 > 0.0 && sc.spawn_r.1 > sc.spawn_r.0, "{name}");
        let mut s = MassiveState::from_bodies(&sc.bodies);
        for _ in 0..400 {
            s.step(&r);
        }
        assert!(s.alive_count() > 0, "{name}");
        for i in 0..s.len() {
            assert!(s.x[i].is_finite() && s.vx[i].is_finite(), "{name} body {i}");
        }
    }
    assert!(build("nope", 1, &RandomOpts::default()).is_err());
}

#[test]
fn same_seed_same_world_different_seed_different_world() {
    let o = RandomOpts::default();
    let a = MassiveState::from_bodies(&build("random", 42, &o).unwrap().bodies).hash();
    let b = MassiveState::from_bodies(&build("random", 42, &o).unwrap().bodies).hash();
    let c = MassiveState::from_bodies(&build("random", 43, &o).unwrap().bodies).hash();
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(build("random", 1, &o).unwrap().bodies.len(), 1000);
}

/// The headline physics check: two simulated years of the solar system at game settings.
#[test]
fn solar_system_orbits_hold() {
    let r = rules();
    let sc = build("solar", 3, &RandomOpts::default()).unwrap();
    let idx = |n: &str| sc.names.iter().find(|x| x.1 == n).unwrap().0 as usize;
    let (sun, earth, moon) = (idx("Sun"), idx("Earth"), idx("Moon"));
    let mut s = MassiveState::from_bodies(&sc.bodies);
    let start: Vec<f64> = (0..s.len()).map(|i| dist(&s, i, sun)).collect();
    let e0 = s.energy(&r);
    let ticks = (2.0 * 365.25 * 86400.0 / r.dt) as u32;
    let (mut moon_lo, mut moon_hi) = (f64::MAX, 0.0f64);
    let mut worst: f64 = 0.0;
    for t in 0..ticks {
        assert!(s.step(&r).is_empty(), "nothing should collide");
        if t % 16 == 0 {
            let d = dist(&s, moon, earth);
            moon_lo = moon_lo.min(d);
            moon_hi = moon_hi.max(d);
            for i in 0..s.len() {
                if i != sun && i != moon {
                    worst = worst.max(((dist(&s, i, sun) - start[i]) / start[i]).abs());
                }
            }
        }
    }
    assert_eq!(s.alive_count(), 10);
    assert!(worst < 0.02, "a planet drifted {:.2}% from its circular orbit", worst * 100.0);
    assert!(moon_lo > 3.3e8 && moon_hi < 4.4e8, "Moon wandered: {moon_lo:.3e} .. {moon_hi:.3e}");
    let drift = ((s.energy(&r) - e0) / e0).abs();
    assert!(drift < 1e-6, "energy drift {drift:e}");
}

#[test]
fn figure_eight_survives_a_full_period() {
    let r = rules();
    let sc = build("figure8", 5, &RandomOpts::default()).unwrap();
    let mut s = MassiveState::from_bodies(&sc.bodies);
    let period_ticks = (6.3259 * (1.0e33f64 / (r.g * 1.0e30)).sqrt() / r.dt) as u32;
    let start = (s.x[0], s.y[0]);
    for _ in 0..period_ticks {
        s.step(&r);
        for i in 0..3 {
            assert!(s.alive[i]);
            assert!((s.x[i].powi(2) + s.y[i].powi(2)).sqrt() < 1.5e11, "star {i} escaped");
        }
    }
    let back = ((s.x[0] - start.0).powi(2) + (s.y[0] - start.1).powi(2)).sqrt();
    assert!(back < 5.0e9, "star 0 should return to its start, off by {back:.3e} m");
}

#[test]
fn runaway_pair_accelerates_together() {
    let r = rules();
    let sc = build("runaway", 5, &RandomOpts::default()).unwrap();
    let idx = |n: &str| sc.names.iter().find(|x| x.1 == n).unwrap().0 as usize;
    let (hare, hound) = (idx("Hare"), idx("Hound"));
    assert!(sc.bodies[hound].mass < 0.0);
    let mut s = MassiveState::from_bodies(&sc.bodies);
    let sep0 = dist(&s, hare, hound);
    for _ in 0..600 {
        s.step(&r);
    }
    assert!(s.alive[hare] && s.alive[hound]);
    assert!(s.vx[hare] > 1.0e4, "pair should have self-accelerated, vx = {}", s.vx[hare]);
    assert!((s.vx[hare] - s.vx[hound]).abs() < 0.2 * s.vx[hare]);
    let sep = dist(&s, hare, hound);
    assert!(sep > 0.5 * sep0 && sep < 2.0 * sep0, "separation {sep0:.2e} -> {sep:.2e}");
}

#[test]
fn repulsor_has_negative_bodies_and_stays_finite() {
    let r = rules();
    let sc = build("repulsor", 9, &RandomOpts::default()).unwrap();
    let neg = sc.bodies.iter().filter(|b| b.mass < 0.0).count();
    assert!(neg > 150 && neg < 350, "{neg}");
    let mut s = MassiveState::from_bodies(&sc.bodies);
    for _ in 0..300 {
        s.step(&r);
    }
    assert!(s.alive_count() > 500);
}
