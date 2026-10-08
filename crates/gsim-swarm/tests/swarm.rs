use gsim_swarm::runner::{Command, Runner};
use gsim_swarm::scenario::{self, Params, Structure, SCENARIOS};
use gsim_swarm::{Bodies, Engine, Level, Mode, Sim};
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
                assert_eq!(engine.mode, Mode::Tree);
                let error = engine.force_error(&bodies, 400);
                assert!(error < limit, "{} {name} theta {theta}: error {error:.2e}", level.name());
            }
        }
    }
}

#[test]
fn a_few_thousand_bodies_are_summed_pair_by_pair() {
    for level in Level::available() {
        let mut bodies = small("galaxy", 3000.0).bodies;
        let mut engine = Engine::with_level(level, G, 1.0e6);
        // The opening angle must not matter here.
        engine.theta = 1.2;
        engine.prime(&mut bodies);
        assert_eq!(engine.mode, Mode::Pairwise);
        let error = engine.force_error(&bodies, 300);
        assert!(error < 2.0e-4, "{}: error {error:.2e}", level.name());
    }
}

#[test]
fn a_planet_keeps_its_orbit_precisely() {
    let mut b = Bodies::default();
    let (star, r) = (2.0e30, 1.5e11);
    let v = (G * star / r).sqrt();
    b.push(0.0, 0.0, 0.0, 0.0, star, 7.0e8, 0);
    b.push(r, 0.0, 0.0, v, 6.0e24, 6.4e6, 0);
    let mut engine = Engine::new(G, 1.0e6);
    // Three years in steps of six hours.
    for _ in 0..4400 {
        engine.step(&mut b, 21_600.0);
    }
    assert_eq!(engine.mode, Mode::Precise);
    let (s, p) = (b.locate(0).unwrap(), b.locate(1).unwrap());
    let d = ((b.x[p] - b.x[s]).powi(2) + (b.y[p] - b.y[s]).powi(2)).sqrt();
    assert!((d / r - 1.0).abs() < 1.0e-4, "orbit radius drifted to {:.6} of the original", d / r);
    // It also knows how fast that orbit turns.
    engine.dt_hint = 1.0e9;
    engine.step(&mut b, 21_600.0);
    let rate = (G * (star + 6.0e24) / r.powi(3)).sqrt();
    assert!((engine.omega2.sqrt() / rate - 1.0).abs() < 0.01, "{} vs {rate}", engine.omega2.sqrt());
}

#[test]
fn tight_orbits_are_found_in_a_crowd() {
    // A planet with a close moon, far out in a galaxy of thousands.
    for count in [3000.0, 12_000.0] {
        let mut b = small("galaxy", count).bodies;
        let (planet, d) = (5.0e26, 4.0e8);
        let v = (G * planet / d).sqrt();
        b.push(3.0e11, 0.0, 0.0, 0.0, planet, 6.0e7, 0);
        b.push(3.0e11 + d, 0.0, 0.0, v, 1.0e22, 1.0e6, 0);
        let mut engine = Engine::new(G, 1.0e6);
        engine.dt_hint = 86_400.0;
        engine.prime(&mut b);
        let rate = (G * planet / d.powi(3)).sqrt();
        if engine.mode != Mode::Tree {
            // With every pair looked at, chance pairings of small bodies count as well.
            assert!(engine.omega2.sqrt() > 0.95 * rate, "{count}: found {:.3e}, the moon turns at {rate:.3e}", engine.omega2.sqrt());
            continue;
        }
        assert!((engine.omega2.sqrt() / rate - 1.0).abs() < 0.05, "{count}: found {:.3e}, the moon turns at {rate:.3e}", engine.omega2.sqrt());
        // A stranger flying past just as close does not count.
        let i = b.locate(b.next_id - 1).unwrap();
        b.vy[i] = 40.0 * v;
        engine.invalidate();
        engine.prime(&mut b);
        assert!(engine.omega2.sqrt() < 0.7 * rate, "{count}: a fly-by was taken for an orbit");
    }
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
    assert_eq!(b.len(), 2, "the absorbed body is dropped");
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
fn every_scenario_builds_at_its_limits() {
    for sc in SCENARIOS {
        for pick in [0, 1, 2] {
            let mut p = Params::new();
            for spec in sc.params {
                let v = [spec.default, spec.min, spec.max][pick];
                p.insert(spec.key.to_string(), if spec.key == "count" { v.min(3000.0) } else { v });
            }
            let setup = scenario::build(sc.name, 5, &p).unwrap_or_else(|e| panic!("{} #{pick}: {e}", sc.name));
            let mut sim = Sim::new(setup);
            for _ in 0..20 {
                sim.step(600.0);
            }
            let b = sim.bodies.read().unwrap();
            assert!(b.len() >= 5 || sc.name == "empty", "{} #{pick} lost its bodies", sc.name);
            assert!(b.x.iter().chain(&b.y).chain(&b.vx).chain(&b.vy).all(|v| v.is_finite()), "{} #{pick}", sc.name);
            assert!(b.ax.iter().chain(&b.ay).all(|v| v.is_finite()), "{} #{pick}", sc.name);
        }
    }
    assert!(scenario::build("galaxy", 1, &params(&[("nope", 1.0)])).is_err());
    assert!(scenario::build("galaxy", 1, &params(&[("heat", 9.0)])).is_err());
    assert!(scenario::build("nope", 1, &Params::new()).is_err());
}

#[test]
fn the_world_can_be_edited() {
    // From nothing, through every mode, and back.
    let mut sim = Sim::new(scenario::build("empty", 1, &Params::new()).unwrap());
    sim.step(100.0);
    let mut star = Bodies::default();
    star.push(0.0, 0.0, 0.0, 0.0, 2.0e30, 7.0e8, 0);
    sim.add(&star);
    sim.add(&scenario::structure(Structure::Ring, 3, 500, 2.0e11, 1.0e26, 2.0e30, 1));
    sim.step(600.0);
    assert_eq!(sim.engine.mode, Mode::Pairwise);
    sim.add(&scenario::structure(Structure::Galaxy, 4, 6000, 5.0e10, 1.0e29, 0.0, 2));
    sim.add(&scenario::structure(Structure::Cloud, 5, 500, 2.0e10, 1.0e27, 0.0, 3));
    for _ in 0..5 {
        sim.step(600.0);
    }
    assert_eq!(sim.engine.mode, Mode::Tree);
    {
        let b = sim.bodies.read().unwrap();
        // All but the few that have merged meanwhile.
        assert!((6980..=7001).contains(&b.len()), "{} bodies", b.len());
        let mut ids = b.id.clone();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), b.len(), "new bodies get ids of their own");
        // The ring really circles the star.
        let ring: Vec<usize> = (0..b.len()).filter(|i| b.group[*i] == 1).collect();
        let far = ring.iter().map(|i| (b.x[*i].powi(2) + b.y[*i].powi(2)).sqrt()).fold(0.0, f64::max);
        assert!(ring.len() > 490 && far < 2.6e11, "{} ring bodies, the farthest at {far:.3e}", ring.len());
    }

    // Shattering keeps the mass and scatters the pieces.
    let mass_before = sim.bodies.read().unwrap().total_mass();
    assert!(sim.shatter(0, 40, 1.5));
    assert!(!sim.shatter(0, 40, 1.5), "it is gone now");
    for _ in 0..3 {
        sim.step(60.0);
    }
    let b = sim.bodies.read().unwrap();
    assert!((b.total_mass() / mass_before - 1.0).abs() < 1e-5);
    assert!(b.len() >= 6980 + 30, "the pieces fell straight back together: {} bodies", b.len());
    let id = b.id[0];
    drop(b);

    assert!(sim.edit(id, 3.0e25, 2.0e7, 12.0, -7.0));
    {
        let b = sim.bodies.read().unwrap();
        let i = b.locate(id).unwrap();
        assert_eq!((b.m[i], b.r[i], b.vx[i], b.vy[i]), (3.0e25, 2.0e7, 12.0, -7.0));
    }
    assert!(sim.remove(id) && !sim.remove(id));
    assert!(!sim.edit(id, 1.0, 1.0, 0.0, 0.0));
    let gone = sim.erase(0.0, 0.0, 1.0e11);
    assert!(gone > 6000, "erased {gone}");
    sim.step(600.0);
    assert_eq!(sim.engine.mode, Mode::Pairwise);
    sim.erase(0.0, 0.0, 1.0e13);
    assert!(sim.bodies.read().unwrap().is_empty());
    sim.step(600.0);
}

#[test]
fn worlds_survive_being_saved() {
    let mut sim = Sim::new(small("galaxy", 5000.0));
    for _ in 0..3 {
        sim.step(600.0);
    }
    let original = sim.bodies.read().unwrap().clone();
    let mut file = Vec::new();
    original.write(&mut file).unwrap();
    let back = Bodies::read(&mut file.as_slice()).unwrap();
    assert_eq!((back.len(), back.time, back.next_id), (original.len(), original.time, original.next_id));
    assert!(back.x == original.x && back.vy == original.vy && back.m == original.m && back.id == original.id && back.group == original.group);
    assert!(Bodies::read(&mut &file[..file.len() - 3]).is_err());
    assert!(Bodies::read(&mut &b"nonsense"[..]).is_err());
    // A loaded world carries on from where it was.
    sim.replace(back);
    assert_eq!(sim.time, original.time);
    sim.step(600.0);
    assert!((sim.bodies.read().unwrap().time - original.time - 600.0).abs() < 1e-6);
}

#[test]
fn watched_bodies_are_followed_through_merges() {
    let mut b = Bodies::default();
    b.push(0.0, 0.0, 0.0, 0.0, 1.0e24, 5.0e6, 0);
    b.push(6.0e6, 0.0, 0.0, 0.0, 3.0e24, 5.0e6, 0);
    for k in 0..300 {
        b.push(1.0e9 + 1.0e8 * k as f64, 3.0e9, 0.0, 0.0, 1.0e20, 1.0e5, 0);
    }
    let mut sim = Sim::new(scenario::Setup { bodies: b, ..small("cloud", 1000.0) });
    sim.watch = vec![0];
    sim.step(1.0);
    assert_eq!(sim.watch, vec![1]);
    assert_eq!(sim.merges_total, 1);
}

#[test]
fn the_runner_holds_the_pace_whatever_a_step_costs() {
    for count in [300.0, 3000.0, 30_000.0] {
        let pace = 86_400.0;
        let runner = Runner::start(Sim::new(scenario::build("cloud", 1, &params(&[("count", count), ("heat", 0.2), ("rotation", 1.0)])).unwrap()), pace, 4);
        std::thread::sleep(Duration::from_millis(400));
        let from = (runner.published.lock().unwrap().time, std::time::Instant::now());
        std::thread::sleep(Duration::from_millis(1200));
        let p = runner.published.lock().unwrap();
        let managed = (p.time - from.0) / from.1.elapsed().as_secs_f64();
        // Either on pace, or knowingly held back by a tight orbit.
        assert!(p.stats.limited || (managed / pace - 1.0).abs() < 0.25, "{count} bodies: {managed:.0} s/s of {pace}, {} steps/s", p.stats.steps_per_s);
        assert!(managed > 0.0);
        assert!((p.stats.achieved / managed - 1.0).abs() < 0.5, "says {} but did {managed}", p.stats.achieved);
        assert!(runner.bodies.read().unwrap().time >= p.time);
    }
}

#[test]
fn the_runner_slows_down_for_a_tight_orbit() {
    let mut b = small("cloud", 2000.0).bodies;
    // A close pair of stars: one turn takes them a quarter of an hour.
    let (m, d) = (1.0e30, 5.0e8);
    let v = (G * m / (2.0 * d)).sqrt();
    b.push(1.0e12 - 0.5 * d, 0.0, 0.0, -v, m, 5.0e7, 0);
    b.push(1.0e12 + 0.5 * d, 0.0, 0.0, v, m, 5.0e7, 0);
    let runner = Runner::start(Sim::new(scenario::Setup { bodies: b, ..small("cloud", 1000.0) }), 86_400.0 * 30.0, 4);
    std::thread::sleep(Duration::from_millis(800));
    let p = runner.published.lock().unwrap();
    assert!(p.stats.limited, "a step of {} s went unquestioned", p.stats.dt);
    let turn = 1.0 / (G * 2.0 * m / d.powi(3)).sqrt();
    assert!(p.stats.dt < 0.3 * turn, "steps of {} s for an orbit that turns a radian in {turn} s", p.stats.dt);
    assert!(p.stats.achieved < 0.5 * p.pace);
}

#[test]
fn the_runner_pauses_edits_and_goes_back() {
    let runner = Runner::start(Sim::new(small("cloud", 4000.0)), 86_400.0, 4);
    std::thread::sleep(Duration::from_millis(200));
    runner.send(Command::Pace(0.0));
    std::thread::sleep(Duration::from_millis(100));
    let (paused_at, bodies) = {
        let p = runner.published.lock().unwrap();
        (p.time, p.stats.bodies)
    };
    // Edits work while time stands still.
    let mut star = Bodies::default();
    star.push(9.0e11, 0.0, 0.0, 0.0, 2.0e30, 7.0e8, 0);
    runner.send(Command::Add(Box::new(star)));

    let dir = std::env::temp_dir().join(format!("gsim-swarm-test-{}", std::process::id()));
    runner.send(Command::Save(dir.join("world.gsw")));
    std::thread::sleep(Duration::from_millis(200));
    {
        let p = runner.published.lock().unwrap();
        assert_eq!((p.time, p.pace), (paused_at, 0.0));
        // The star, less whatever merged in the step that was still running.
        assert!((bodies - 5..=bodies + 1).contains(&p.stats.bodies), "{} bodies, there were {bodies}", p.stats.bodies);
        assert!(p.heaviest.first().is_some_and(|h| h.mass == 2.0e30 && h.x == 9.0e11));
        assert_eq!(p.notice.as_deref(), Some("Saved world"));
    }
    runner.send(Command::Erase { x: 0.0, y: 0.0, r: 1.0e13 });
    runner.send(Command::Load(dir.join("world.gsw")));
    runner.send(Command::Pace(3600.0));
    std::thread::sleep(Duration::from_millis(300));
    let p = runner.published.lock().unwrap();
    // Give or take what merged in the moment since.
    assert!((bodies - 40..=bodies + 1).contains(&p.stats.bodies), "{} bodies after loading {}", p.stats.bodies, bodies + 1);
    assert!(p.time > paused_at && p.pace == 3600.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_reader_never_waits_long_for_a_small_world() {
    use std::sync::atomic::Ordering;
    for count in [60.0, 1000.0] {
        let runner = Runner::start(Sim::new(small("cloud", count)), 86_400.0, 4);
        std::thread::sleep(Duration::from_millis(200));
        let mut worst = [Duration::ZERO; 2];
        for (k, polite) in [false, true].into_iter().enumerate() {
            for _ in 0..100 {
                let from = std::time::Instant::now();
                runner.reading.store(polite, Ordering::Release);
                {
                    let _p = runner.published.lock().unwrap();
                    let b = runner.bodies.read().unwrap();
                    std::hint::black_box(b.len());
                    std::thread::sleep(Duration::from_micros(300));
                }
                runner.reading.store(false, Ordering::Release);
                worst[k] = worst[k].max(from.elapsed());
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        eprintln!("{count} bodies: worst wait {:?} unannounced, {:?} announced", worst[0], worst[1]);
        assert!(worst[1] < Duration::from_millis(8), "{count} bodies: a reader waited {:?}", worst[1]);
    }
}

#[test]
fn only_bodies_that_are_really_leaving_fade_away() {
    // A moon outruns the star's escape speed for half of every turn around its planet, yet
    // is held by the planet.
    let mut b = Bodies::default();
    let (sun, planet, far, near) = (2.0e30, 1.9e27, 7.8e11, 4.2e8);
    let (around_sun, around_planet) = ((G * sun / far).sqrt(), (G * planet / near).sqrt());
    b.push(0.0, 0.0, 0.0, 0.0, sun, 7.0e8, 0);
    b.push(far, 0.0, 0.0, around_sun, planet, 7.0e7, 0);
    b.push(far + near, 0.0, 0.0, around_sun + around_planet, 9.0e22, 1.8e6, 0);
    assert!(around_sun + around_planet > (2.0 * G * sun / far).sqrt());
    let mut engine = Engine::new(G, 0.0);
    for _ in 0..20_000 {
        engine.step(&mut b, 200.0);
        engine.leave(&mut b);
    }
    assert_eq!(b.len(), 3);
    assert!(b.fade.iter().all(|f| *f == 0.0), "{:?}", b.fade);

    for count in [100.0, 3000.0, 20_000.0] {
        let mut b = small("cloud", count).bodies;
        let reach = (0..b.len()).map(|i| b.x[i].hypot(b.y[i])).fold(0.0, f64::max);
        let fast = 3.0 * (2.0 * G * b.total_mass() / (2.0 * reach)).sqrt();
        b.push(2.0 * reach, 0.0, fast, 0.0, 1.0e20, 1.0e5, 0);
        let runaway = *b.id.last().unwrap();
        let before = b.len();
        let mut engine = Engine::new(G, 0.0);
        let (mut faded, mut gone) = (false, 0);
        for step in 0..2000 {
            engine.step(&mut b, 5.0);
            engine.leave(&mut b);
            gone += engine.stats.removed;
            match b.locate(runaway).filter(|i| b.alive(*i)) {
                Some(i) => faded |= b.fade[i] > 0.0 && b.fade[i] < 1.0,
                None => {
                    assert!(faded, "{count} bodies: gone at step {step} without fading");
                    break;
                }
            }
        }
        assert!(b.locate(runaway).is_none_or(|i| !b.alive(i)), "{count} bodies: the runaway is still there");
        // Nothing in a cloud that has only just begun to fall together is on its way out.
        assert!(gone >= 1 && gone <= 1 + before as u32 / 200, "{count} bodies: {gone} dropped");
    }
}
