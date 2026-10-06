use gsim_core::{GameRules, MassiveState};
use gsim_server::scenario::{build, parse_setting, preset, validate, ParamKind, Params, PRESETS};

fn rules() -> GameRules {
    GameRules::new(86400.0, 60)
}

fn dist(s: &MassiveState, a: usize, b: usize) -> f64 {
    ((s.x[a] - s.x[b]).powi(2) + (s.y[a] - s.y[b]).powi(2)).sqrt()
}

#[test]
fn every_preset_builds_and_runs() {
    let r = rules();
    for p in PRESETS {
        let name = p.name;
        // Keep the big fields small enough to step quickly.
        let params: Params = p.param("count").map(|_| ("count".to_string(), 300.0)).into_iter().collect();
        let sc = build(name, 7, &params).unwrap();
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
    assert!(build("nope", 1, &Params::new()).is_err());
}

#[test]
fn same_seed_same_world_different_seed_different_world() {
    let o = Params::new();
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
    let sc = build("solar", 3, &Params::new()).unwrap();
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
    let sc = build("figure8", 5, &Params::new()).unwrap();
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
    let sc = build("runaway", 5, &Params::new()).unwrap();
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
    let sc = build("repulsor", 9, &Params::new()).unwrap();
    let neg = sc.bodies.iter().filter(|b| b.mass < 0.0).count();
    assert!(neg > 150 && neg < 350, "{neg}");
    let mut s = MassiveState::from_bodies(&sc.bodies);
    for _ in 0..300 {
        s.step(&r);
    }
    assert!(s.alive_count() > 500);
}

fn set(settings: &[&str]) -> Params {
    settings.iter().map(|s| parse_setting(s).unwrap()).collect()
}

#[test]
fn parameters_have_sane_tables() {
    for p in PRESETS {
        assert!(preset(p.name).is_some());
        assert!(validate(p.name, &p.defaults()).is_ok(), "{}", p.name);
        for s in p.params {
            assert!(s.min <= s.default && s.default <= s.max && s.min < s.max, "{} {}", p.name, s.key);
            assert!(s.kind != ParamKind::Log || s.min > 0.0, "{} {}: a logarithmic scale cannot reach 0", p.name, s.key);
            assert!(!s.label.is_empty() && !s.help.is_empty() && s.key != "seed", "{} {}", p.name, s.key);
            assert_eq!(p.params.iter().filter(|o| o.key == s.key).count(), 1, "{} {}", p.name, s.key);
        }
    }
}

/// No corner of the parameter space may produce a world that is not a number.
#[test]
fn every_preset_survives_its_extremes() {
    let r = rules();
    for p in PRESETS {
        let lows: Params = p.params.iter().map(|s| (s.key.to_string(), s.min)).collect();
        let highs: Params = p.params.iter().map(|s| (s.key.to_string(), s.max)).collect();
        // As many, as heavy and as tightly packed as allowed (and the reverse).
        let mut dense = highs.clone();
        let mut sparse = lows.clone();
        for key in ["spread", "separation", "size", "scale"] {
            if let Some(s) = p.param(key) {
                dense.insert(key.to_string(), s.min);
                sparse.insert(key.to_string(), s.max);
            }
        }
        for (what, params) in [("defaults", Params::new()), ("lows", lows), ("highs", highs), ("dense", dense), ("sparse", sparse)] {
            let sc = build(p.name, 3, &params).unwrap_or_else(|e| panic!("{} {what}: {e}", p.name));
            assert!(!sc.bodies.is_empty() && sc.bodies.len() <= 5000, "{} {what}: {} bodies", p.name, sc.bodies.len());
            assert!(sc.spawn_r.0 > 0.0 && sc.spawn_r.1 > sc.spawn_r.0 && sc.spawn_r.1.is_finite(), "{} {what}", p.name);
            assert!(sc.escape_radius().is_finite(), "{} {what}", p.name);
            for (i, b) in sc.bodies.iter().enumerate() {
                let finite = [b.x, b.y, b.vx, b.vy, b.mass, b.radius].iter().all(|x| x.is_finite());
                assert!(finite && b.radius > 0.0 && b.mass != 0.0, "{} {what}: body {i} is {b:?}", p.name);
            }
            let mut s = MassiveState::from_bodies(&sc.bodies);
            for _ in 0..3 {
                s.step(&r);
            }
            assert!(s.alive_count() > 0, "{} {what}", p.name);
            for i in (0..s.len()).filter(|&i| s.alive[i]) {
                let finite = [s.x[i], s.y[i], s.vx[i], s.vy[i], s.mass[i], s.radius[i]].iter().all(|x| x.is_finite());
                assert!(finite, "{} {what}: body {i} after stepping", p.name);
            }
        }
    }
}

#[test]
fn parameters_change_the_world() {
    assert_eq!(build("random", 1, &set(&["count=50"])).unwrap().bodies.len(), 50);
    assert_eq!(build("binary", 1, &set(&["debris=10"])).unwrap().bodies.len(), 12);
    assert_eq!(build("figure8", 1, &set(&["spectators=0"])).unwrap().bodies.len(), 3);
    assert_eq!(build("runaway", 1, &set(&["planets=2", "debris=0"])).unwrap().bodies.len(), 5);
    let moonless = build("solar", 1, &set(&["moons=0"])).unwrap();
    assert!(moonless.bodies.len() == 9 && !moonless.names.iter().any(|n| n.1 == "Moon"));

    let extent = |name: &str, settings: &[&str]| build(name, 1, &set(settings)).unwrap().extent();
    assert!(extent("random", &["spread=200Gm"]) > 3.0 * extent("random", &[]));
    assert!((extent("solar", &["scale=2"]) / extent("solar", &[]) - 2.0).abs() < 0.01);
    assert!((extent("figure8", &["size=3e11", "spectators=0"]) / extent("figure8", &["spectators=0"]) - 3.0).abs() < 1e-9);

    let star = build("random", 1, &set(&["star_mass=1e30"])).unwrap();
    assert_eq!((star.bodies[0].mass, star.names[0].1.as_str(), star.bodies.len()), (1.0e30, "Star", 1000));
    let sc = build("repulsor", 1, &set(&["negative=0"])).unwrap();
    assert!(sc.bodies.iter().all(|b| b.mass > 0.0));
    let sc = build("runaway", 1, &set(&["pair_mass=5e26"])).unwrap();
    assert_eq!(sc.bodies.last().unwrap().mass, -5.0e26);
    // Defaults spelled out are the same world as no settings at all.
    for p in PRESETS {
        let hash = |params: &Params| MassiveState::from_bodies(&build(p.name, 4, params).unwrap().bodies).hash();
        assert_eq!(hash(&p.defaults()), hash(&Params::new()), "{}", p.name);
    }
}

/// A scaled solar system is still a solar system: orbits stay round and the Moon stays put.
#[test]
fn scaled_solar_system_stays_bound() {
    let r = rules();
    for settings in [["scale=0.2", "star_mass=5e30"], ["scale=5", "star_mass=2e29"], ["scale=0.2", "star_mass=2e29"], ["scale=5", "star_mass=5e30"]] {
        let sc = build("solar", 3, &set(&settings)).unwrap();
        let idx = |n: &str| sc.names.iter().find(|x| x.1 == n).unwrap().0 as usize;
        let (sun, earth, moon) = (idx("Sun"), idx("Earth"), idx("Moon"));
        let mut s = MassiveState::from_bodies(&sc.bodies);
        let start: Vec<f64> = (0..s.len()).map(|i| dist(&s, i, sun)).collect();
        let moon0 = dist(&s, moon, earth);
        // A few orbits of Mercury, many of the Moon.
        let year = std::f64::consts::TAU * (start[earth].powi(3) / (r.g * sc.bodies[sun].mass)).sqrt();
        for _ in 0..(year / r.dt) as u32 {
            assert!(s.step(&r).is_empty(), "{settings:?}: nothing should collide");
        }
        for i in (0..s.len()).filter(|&i| i != sun && i != moon) {
            let drift = ((dist(&s, i, sun) - start[i]) / start[i]).abs();
            assert!(drift < 0.03, "{settings:?}: body {i} drifted {:.2}%", drift * 100.0);
        }
        let d = dist(&s, moon, earth);
        assert!(d > 0.7 * moon0 && d < 1.4 * moon0, "{settings:?}: Moon went from {moon0:.3e} to {d:.3e}");
    }
}

#[test]
fn settings_are_parsed_and_checked() {
    assert_eq!(parse_setting("spread=80Gm").unwrap(), ("spread".to_string(), 8.0e10));
    assert_eq!(parse_setting("mass_max=2e25").unwrap(), ("mass_max".to_string(), 2.0e25));
    for bad in ["spread", "=3", "spread=", "spread=wide", "spread=inf", "spread=NaN"] {
        assert!(parse_setting(bad).is_err(), "{bad}");
    }

    assert!(validate("random", &set(&["count=5000", "rotation=0"])).is_ok());
    let err = |name: &str, settings: &[&str]| validate(name, &set(settings)).unwrap_err();
    assert_eq!(err("random", &["count=5001"]), "count=5001 is out of range (1 to 5000)");
    assert_eq!(err("random", &["spread=1"]), "spread=1 is out of range (1e9 to 1e13 m)");
    assert_eq!(err("random", &["count=2.5"]), "count=2.5 must be a whole number");
    assert_eq!(err("solar", &["count=5"]), "preset solar has no parameter 'count' (it has: star_mass, scale, moons)");
    assert!(err("nowhere", &[]).starts_with("unknown preset 'nowhere'"));
    // Building checks too.
    assert!(build("random", 1, &set(&["negative=0.5"])).is_err());
    assert!(build("binary", 1, &set(&["debris=1e9"])).is_err());
}
