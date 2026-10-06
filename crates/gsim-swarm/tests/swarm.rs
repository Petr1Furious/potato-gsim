use gsim_core::{Particle, ShipInput};
use gsim_swarm::runner::{Command, Runner};
use gsim_swarm::scenario::{self, Params, SCENARIOS};
use gsim_swarm::sim::Event;
use gsim_swarm::{Bodies, Engine, Level, Local, Sim};
use std::time::Duration;

const G: f64 = 6.67430e-11;

fn params(pairs: &[(&str, f64)]) -> Params {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn small(name: &str, count: f64) -> scenario::Setup {
    scenario::build(name, 7, &params(&[("count", count)])).unwrap()
}

fn momentum(b: &Bodies) -> (f64, f64, f64) {
    let mut out = (0.0, 0.0, 0.0);
    for i in 0..b.len() {
        let m = b.m[i] as f64;
        out = (out.0 + m, out.1 + m * b.vx[i], out.2 + m * b.vy[i]);
    }
    out
}

#[test]
fn forces_match_exact_sums_on_every_instruction_set() {
    for level in Level::available() {
        for (theta, limit) in [(0.3f32, 4.0e-4), (0.7, 5.0e-3)] {
            for name in ["galaxy", "cloud"] {
                let mut bodies = small(name, 6000.0).bodies;
                let mut engine = Engine::with_level(level, G, 1.0e6);
                engine.theta = theta;
                engine.prime(&mut bodies);
                let error = engine.force_error(&bodies, 400);
                assert!(error < limit, "{} {name} theta {theta}: error {error:.2e}", level.name());
            }
        }
    }
}

#[test]
fn levels_agree_with_each_other() {
    let reference = {
        let mut b = small("galaxy", 3000.0).bodies;
        Engine::with_level(Level::Plain, G, 1.0e6).prime(&mut b);
        b
    };
    for level in Level::available() {
        let mut b = small("galaxy", 3000.0).bodies;
        Engine::with_level(level, G, 1.0e6).prime(&mut b);
        assert_eq!(b.id, reference.id);
        for i in 0..b.len() {
            let scale = (reference.ax[i].abs() + reference.ay[i].abs()).max(1e-12);
            assert!((b.ax[i] - reference.ax[i]).abs() / scale < 2.0e-3, "{} body {i}", level.name());
        }
    }
}

#[test]
fn a_planet_keeps_its_orbit() {
    let mut b = Bodies::default();
    let (star, r) = (2.0e30, 1.5e11);
    let v = (G * star / r).sqrt();
    b.push(0.0, 0.0, 0.0, 0.0, star, 7.0e8, 0);
    b.push(r, 0.0, 0.0, v, 6.0e24, 6.4e6, 0);
    let mut engine = Engine::new(G, 1.0e6);
    // Three years in steps of 1440 s.
    for _ in 0..65_000 {
        engine.step(&mut b, 1440.0);
    }
    let (s, p) = (b.locate(0).unwrap(), b.locate(1).unwrap());
    let d = ((b.x[p] - b.x[s]).powi(2) + (b.y[p] - b.y[s]).powi(2)).sqrt();
    assert!((d / r - 1.0).abs() < 0.01, "orbit radius drifted to {:.4} of the original", d / r);
}

#[test]
fn overlapping_bodies_merge_into_the_heavier_one() {
    let mut b = Bodies::default();
    b.push(0.0, 0.0, 10.0, 0.0, 1.0e24, 5.0e6, 0);
    b.push(6.0e6, 0.0, -30.0, 0.0, 3.0e24, 5.0e6, 1);
    b.push(5.0e9, 5.0e9, 0.0, 0.0, 1.0e22, 1.0e6, 0);
    let before = momentum(&b);
    let mut engine = Engine::new(G, 1.0e6);
    let merges = engine.step(&mut b, 1.0);
    assert_eq!(merges.len(), 1);
    assert_eq!((merges[0].survivor, merges[0].absorbed), (1, 0));
    engine.step(&mut b, 1.0);
    assert_eq!(b.len(), 2, "the absorbed body is dropped at the next sort");
    let i = b.locate(1).unwrap();
    assert_eq!(b.group[i], 1);
    assert!((b.m[i] as f64 - 4.0e24).abs() < 1.0e18);
    assert!((b.r[i] as f64 - 5.0e6 * 2f64.cbrt()).abs() < 10.0);
    let after = momentum(&b);
    assert!((after.0 - before.0).abs() / before.0 < 1e-6);
    assert!((after.1 - before.1).abs() / before.1.abs() < 1e-3);
    assert!(b.locate(0).is_none());
}

#[test]
fn a_collapsing_cloud_merges_and_keeps_its_mass_and_momentum() {
    let setup = scenario::build("cloud", 3, &params(&[("count", 20_000.0), ("size", 40.0), ("rotation", 0.1)])).unwrap();
    let mut b = setup.bodies;
    let mut engine = Engine::new(G, 1.0e6);
    engine.prime(&mut b);
    let before = momentum(&b);
    let scale: f64 = (0..b.len()).map(|i| b.m[i] as f64 * b.vx[i].abs()).sum();
    let mut merged = 0;
    for _ in 0..600 {
        merged += engine.step(&mut b, 1440.0).len();
    }
    engine.step(&mut b, 1440.0);
    assert!(merged > 500, "only {merged} merges");
    assert_eq!(b.len(), 20_000 - merged);
    let mut ids = b.id.clone();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), b.len(), "ids stay unique");
    let after = momentum(&b);
    assert!((after.0 - before.0).abs() / before.0 < 1e-4, "mass {} -> {}", before.0, after.0);
    assert!((after.1 - before.1).abs() / scale < 1e-3 && (after.2 - before.2).abs() / scale < 1e-3, "momentum drifted");
    assert!(b.x.iter().chain(&b.vx).all(|v| v.is_finite()));
}

#[test]
fn the_local_list_accounts_for_the_whole_world() {
    let mut b = small("galaxy", 30_000.0).bodies;
    let mut engine = Engine::new(G, 1.0e6);
    engine.prime(&mut b);
    let total: f64 = b.m.iter().map(|m| *m as f64).sum();
    let mut local = Local::default();
    let wanted = b.id[b.len() / 2];
    engine.local(&b, 4.0e10, 1.0e10, 0.5, 2.0e9, &[wanted], &mut local);
    let listed: f64 = local.mass.iter().sum();
    assert!((listed / total - 1.0).abs() < 1e-5, "{listed} of {total}");
    assert!(local.len() < 1500, "{} entries", local.len());
    assert!(local.slot_of(wanted).is_some());
    // Its pull agrees with summing every body.
    let (ax, ay) = local.accel_at(4.0e10, 1.0e10, G, 1.0e6);
    let (mut ex, mut ey) = (0.0, 0.0);
    for i in 0..b.len() {
        let (dx, dy) = (b.x[i] - 4.0e10, b.y[i] - 1.0e10);
        let d2 = dx * dx + dy * dy + 1.0e12;
        let f = G * b.m[i] as f64 / (d2 * d2.sqrt());
        ex += dx * f;
        ey += dy * f;
    }
    let error = ((ax - ex).powi(2) + (ay - ey).powi(2)).sqrt() / (ex * ex + ey * ey).sqrt();
    assert!(error < 0.02, "error {error:.2e}");
}

#[test]
fn every_scenario_builds_at_its_limits() {
    for sc in SCENARIOS {
        for pick in [0, 1, 2] {
            let mut p = Params::new();
            for spec in sc.params {
                let v = [spec.default, spec.min, spec.max][pick];
                p.insert(spec.key.to_string(), if spec.key == "count" { v.min(3000.0) } else { v });
            }
            let setup = scenario::build(sc.name, 5, &p).unwrap_or_else(|e| panic!("{} #{pick}: {e}", sc.name));
            let mut sim = Sim::new(setup, 1);
            for _ in 0..20 {
                sim.step(ShipInput::default());
            }
            let b = sim.bodies.read().unwrap();
            assert!(b.len() > 100, "{} #{pick} lost its bodies", sc.name);
            assert!(b.x.iter().chain(&b.y).chain(&b.vx).chain(&b.vy).all(|v| v.is_finite()), "{} #{pick}", sc.name);
            assert!(b.ax.iter().chain(&b.ay).all(|v| v.is_finite()), "{} #{pick}", sc.name);
        }
    }
    assert!(scenario::build("galaxy", 1, &params(&[("nope", 1.0)])).is_err());
    assert!(scenario::build("galaxy", 1, &params(&[("heat", 9.0)])).is_err());
    assert!(scenario::build("nope", 1, &Params::new()).is_err());
}

#[test]
fn a_ship_spawns_on_an_orbit_and_stays_in_the_galaxy() {
    let mut sim = Sim::new(small("galaxy", 20_000.0), 11);
    let (_, events) = sim.step(ShipInput::default());
    assert!(matches!(events[..], [Event::Spawned]));
    let start = sim.ship.unwrap().p;
    let r0 = (start.x * start.x + start.y * start.y).sqrt();
    let (mut lo, mut hi) = (r0, r0);
    for _ in 0..6000 {
        sim.step(ShipInput::default());
        let Some(s) = sim.ship else { break };
        let r = (s.p.x * s.p.x + s.p.y * s.p.y).sqrt();
        (lo, hi) = (lo.min(r), hi.max(r));
    }
    assert!(lo > 0.6 * r0 && hi < 1.6 * r0, "orbit wandered from {r0:.3e} to {lo:.3e}..{hi:.3e}");
    assert!(sim.local.len() > 50 && sim.local.len() < 1500);
}

#[test]
fn ships_crash_unless_indestructible() {
    for god in [false, true] {
        let mut sim = Sim::new(small("galaxy", 5000.0), 3);
        sim.god = god;
        sim.step(ShipInput::default());
        // Straight down onto the core from just above it.
        let (x, y) = {
            let b = sim.bodies.read().unwrap();
            let i = b.locate(0).unwrap();
            (b.x[i], b.y[i])
        };
        sim.place(Particle { x: x + 4.0e9, y, vx: -4.0e4, vy: 0.0 });
        let mut crashed = false;
        for _ in 0..400 {
            let (_, events) = sim.step(ShipInput { angle: 0, thrust: 100 });
            crashed |= events.iter().any(|e| matches!(e, Event::Crashed { body: 0, .. }));
        }
        assert_eq!(crashed, !god);
        if god {
            assert_eq!(sim.ship.unwrap().fuel, sim.rules.fuel_max_mmps);
            assert_eq!(sim.deaths, 0);
        } else {
            assert!(sim.deaths >= 1);
        }
    }
}

#[test]
fn watched_bodies_are_followed_through_merges() {
    let mut b = Bodies::default();
    b.push(0.0, 0.0, 0.0, 0.0, 1.0e24, 5.0e6, 0);
    b.push(6.0e6, 0.0, 0.0, 0.0, 3.0e24, 5.0e6, 0);
    for k in 0..200 {
        b.push(1.0e9 + 1.0e8 * k as f64, 3.0e9, 0.0, 0.0, 1.0e20, 1.0e5, 0);
    }
    let setup = scenario::Setup { bodies: b, spawn_around: None, spawn_r: (2.0e9, 3.0e9), ..small("cloud", 1000.0) };
    let mut sim = Sim::new(setup, 1);
    sim.watch = vec![0];
    sim.step(ShipInput::default());
    assert_eq!(sim.watch, vec![1]);
    assert_eq!(sim.merges_total, 1);
}

#[test]
fn the_runner_paces_pauses_and_stops() {
    let runner = Runner::start(Sim::new(small("cloud", 4000.0), 1));
    std::thread::sleep(Duration::from_millis(700));
    let (tick, ship) = {
        let p = runner.published.lock().unwrap();
        (p.tick, p.ship)
    };
    assert!((25..=50).contains(&tick), "ran {tick} ticks in 0.7 s at 60 Hz");
    assert!(ship.is_some());
    assert_eq!(runner.bodies.read().unwrap().tick, tick);
    runner.send(Command::Pace(0.0));
    std::thread::sleep(Duration::from_millis(100));
    let paused_at = runner.published.lock().unwrap().tick;
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(runner.published.lock().unwrap().tick, paused_at);
    runner.send(Command::Pace(4.0 * 86_400.0));
    runner.send(Command::God(true));
    std::thread::sleep(Duration::from_millis(500));
    let p = runner.published.lock().unwrap();
    assert!((15..=40).contains(&(p.tick - paused_at)), "{} ticks in 0.5 s", p.tick - paused_at);
    // Faster time means longer steps, not more of them.
    assert_eq!((p.pace, p.ship_dt), (345_600.0, 5760.0));
    assert_eq!(runner.bodies.read().unwrap().dt, 5760.0);
    assert!(p.god && p.local.is_some() && !p.heaviest.is_empty());
    drop(p);
    drop(runner);
}
