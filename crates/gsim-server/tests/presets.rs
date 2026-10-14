use gsim_core::{GameRules, MassiveState};
use gsim_server::scenario::{build, parse_setting, preset, validate, ParamKind, Params, PRESETS};

fn rules() -> GameRules {
    GameRules::new(86400.0, 60)
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

    let extent = |name: &str, settings: &[&str]| build(name, 1, &set(settings)).unwrap().extent();
    assert!(extent("random", &["spread=200Gm"]) > 3.0 * extent("random", &[]));

    let star = build("random", 1, &set(&["star_mass=1e30"])).unwrap();
    assert_eq!((star.bodies[0].mass, star.names[0].1.as_str(), star.bodies.len()), (1.0e30, "Star", 1000));
    let sc = build("repulsor", 1, &set(&["negative=0"])).unwrap();
    assert!(sc.bodies.iter().all(|b| b.mass > 0.0));
    // Defaults spelled out are the same world as no settings at all.
    for p in PRESETS {
        let hash = |params: &Params| MassiveState::from_bodies(&build(p.name, 4, params).unwrap().bodies).hash();
        assert_eq!(hash(&p.defaults()), hash(&Params::new()), "{}", p.name);
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
    assert_eq!(err("random", &["negative=0.5"]), "preset random has no parameter 'negative' (it has: count, spread, mass_min, mass_max, rotation, star_mass)");
    assert!(err("nowhere", &[]).starts_with("unknown preset 'nowhere'"));
    // Building checks too.
    assert!(build("random", 1, &set(&["negative=0.5"])).is_err());
    assert!(build("repulsor", 1, &set(&["negative=2"])).is_err());
}
