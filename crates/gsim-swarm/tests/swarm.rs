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
        assert!((6940..=7001).contains(&b.len()), "{} bodies", b.len());
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
    let (mass_before, count_before) = (sim.bodies.read().unwrap().total_mass(), sim.bodies.read().unwrap().len());
    assert!(sim.shatter(0, 40, 1.5));
    assert!(!sim.shatter(0, 40, 1.5), "it is gone now");
    for _ in 0..3 {
        sim.step(60.0);
    }
    let b = sim.bodies.read().unwrap();
    assert!((b.total_mass() / mass_before - 1.0).abs() < 1e-5);
    assert!(b.len() >= count_before + 15, "the pieces fell straight back together: {} bodies", b.len());
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
    // A world small enough to be stepped whole follows tight orbits by shortening every step.
    let mut b = small("cloud", 100.0).bodies;
    // A close pair of stars: one turn takes them a quarter of an hour.
    let (m, d) = (1.0e30, 5.0e8);
    let v = (G * m / (2.0 * d)).sqrt();
    b.push(1.0e12 - 0.5 * d, 0.0, 0.0, -v, m, 5.0e7, 0);
    b.push(1.0e12 + 0.5 * d, 0.0, 0.0, v, m, 5.0e7, 0);
    let runner = Runner::start(Sim::new(scenario::Setup { bodies: b, ..small("cloud", 100.0) }), 86_400.0 * 3000.0, 4);
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

#[test]
fn a_moon_takes_steps_of_its_own() {
    // A moon that goes a third of the way round its star in one step of the world.
    let (star, d, dt) = (1.0e30, 5.0e8, 3000.0);
    let v = (G * star / d).sqrt();
    for count in [3000.0, 20_000.0] {
        let mut wander = [0.0f64; 2];
        for (k, own) in [false, true].into_iter().enumerate() {
            let mut b = small("cloud", count).bodies;
            b.push(1.0e12, 0.0, 0.0, 0.0, star, 5.0e7, 0);
            b.push(1.0e12 + d, 0.0, 0.0, v, 1.0e22, 1.0e6, 0);
            // And one let go at rest, which falls into the star within the step.
            b.push(1.0e12, 2.0 * d, 0.0, 0.0, 1.0e22, 1.0e6, 0);
            let (sun, moon, stone) = (b.id[b.len() - 3], b.id[b.len() - 2], b.id[b.len() - 1]);
            let mut engine = Engine::new(G, 0.0);
            engine.own_steps = own;
            let mut fell = false;
            for _ in 0..300 {
                fell |= engine.step(&mut b, dt).iter().any(|m| m.absorbed == stone && m.survivor == sun);
                let (Some(s), Some(m)) = (b.locate(sun), b.locate(moon).filter(|i| b.alive(*i))) else {
                    wander[k] = f64::MAX;
                    break;
                };
                wander[k] = wander[k].max(((b.x[m] - b.x[s]).hypot(b.y[m] - b.y[s]) / d - 1.0).abs());
            }
            if own {
                assert!(fell, "{count} bodies: the falling stone passed through the star");
                assert!(engine.stats.fine >= 1 && engine.stats.most >= 16, "{count} bodies: {} on steps of their own, {} at most", engine.stats.fine, engine.stats.most);
            }
        }
        assert!(wander[0] > 0.5, "{count} bodies: the orbit held without help ({})", wander[0]);
        assert!(wander[1] < 0.05, "{count} bodies: the orbit wandered by {} of its radius", wander[1]);
    }
}

/// Kinetic plus potential energy, summed over every pair.
fn energy(b: &Bodies) -> f64 {
    let mut e = 0.0;
    for i in 0..b.len() {
        let mi = b.m[i] as f64;
        e += 0.5 * mi * (b.vx[i] * b.vx[i] + b.vy[i] * b.vy[i]);
        for j in 0..i {
            e -= G * mi * b.m[j] as f64 / (b.x[i] - b.x[j]).hypot(b.y[i] - b.y[j]).max(1.0);
        }
    }
    e
}

#[test]
fn close_passes_do_not_fling_bodies_away() {
    // A cloud falling together at steps far too long for its close encounters. Left alone,
    // a pull sampled at the closest point of a pass is applied for a whole step and throws
    // bodies out at many times any speed the cloud can give them; on steps of their own, the outcome
    // is that of steps a hundred times shorter (nine bodies above the mark, energy within a
    // hundredth).
    let mut flung = [0usize; 2];
    let mut drift = [0.0f64; 2];
    for (k, own) in [false, true].into_iter().enumerate() {
        let mut b = scenario::build("cloud", 3, &params(&[("count", 3800.0)])).unwrap().bodies;
        let fastest = (0..b.len()).map(|i| b.vx[i].hypot(b.vy[i])).fold(0.0, f64::max);
        let before = energy(&b);
        let mut engine = Engine::new(G, 0.0);
        engine.own_steps = own;
        for _ in 0..1000 {
            engine.step(&mut b, 3000.0);
        }
        flung[k] = (0..b.len()).filter(|&i| b.vx[i].hypot(b.vy[i]) > 5.0 * fastest).count();
        drift[k] = (energy(&b) - before) / before.abs();
    }
    assert!(flung[0] > 200 && drift[0] > 2.0, "left alone: {} flung, energy up by {}", flung[0], drift[0]);
    // (The energy is not kept: merging bodies give theirs up, by more or less as chance has
    // the heavy ones meet.)
    assert!(flung[1] < 40 && drift[1].abs() < 1.0, "on steps of their own: {} flung, energy changed by {}", flung[1], drift[1]);
}

#[test]
fn a_planet_circles_a_tight_pair_of_stars() {
    // Two stars that go round each other several times in one step of the world, and a
    // planet around the pair: it has to find the stars where they are each time it is
    // moved, not where a straight line from the start of the step would put them.
    let (star, apart, dt) = (1.0e30, 4.0e8, 12_000.0);
    let around = (G * star / (2.0 * apart)).sqrt();
    let (far, planet_speed) = (8.0 * apart, (G * 2.0 * star / (8.0 * apart)).sqrt());
    let mut b = small("cloud", 3000.0).bodies;
    b.push(1.0e12 - 0.5 * apart, 0.0, 0.0, -around, star, 3.0e7, 0);
    b.push(1.0e12 + 0.5 * apart, 0.0, 0.0, around, star, 3.0e7, 0);
    b.push(1.0e12 + far, 0.0, 0.0, planet_speed, 1.0e24, 6.0e6, 0);
    let (one, two, planet) = (b.id[b.len() - 3], b.id[b.len() - 2], b.id[b.len() - 1]);
    let turn = std::f64::consts::TAU * apart / (2.0 * around);
    assert!(dt > 2.0 * turn, "the stars should turn more than twice a step, not once in {turn} s");
    let mut engine = Engine::new(G, 0.0);
    let (mut stars, mut orbit) = ((f64::MAX, 0.0f64), (f64::MAX, 0.0f64));
    for _ in 0..400 {
        engine.step(&mut b, dt);
        let (i, j, p) = (b.locate(one).unwrap(), b.locate(two).unwrap(), b.locate(planet).unwrap());
        let d = (b.x[i] - b.x[j]).hypot(b.y[i] - b.y[j]) / apart;
        let r = (b.x[p] - 0.5 * (b.x[i] + b.x[j])).hypot(b.y[p] - 0.5 * (b.y[i] + b.y[j])) / far;
        stars = (stars.0.min(d), stars.1.max(d));
        orbit = (orbit.0.min(r), orbit.1.max(r));
    }
    assert!(stars.0 > 0.93 && stars.1 < 1.07, "the stars went from {} to {} of their distance", stars.0, stars.1);
    assert!(orbit.0 > 0.9 && orbit.1 < 1.1, "the planet went from {} to {} of its distance", orbit.0, orbit.1);
}

/// One step through the phases a large world goes through, however few bodies there are.
fn through(e: &mut Engine, b: &mut Bodies, dt: f64) {
    e.begin(b, dt);
    while e.own(b, std::time::Duration::MAX) {}
    e.end(b);
    e.forces(b);
    e.finish(b);
}

/// Two bodies on an orbit of semi-major axis `a` around the origin, starting furthest apart.
/// Returns their ids, the rate (rad/s) of their mean motion and their relative speed.
fn pair(b: &mut Bodies, heavy: f64, ratio: f64, a: f64, ecc: f64) -> (u32, u32, f64, f64) {
    let light = heavy / ratio;
    let mu = G * (heavy + light);
    let v = (mu * (1.0 - ecc) / (a * (1.0 + ecc))).sqrt();
    let (of_heavy, of_light) = (light / (heavy + light), heavy / (heavy + light));
    let first = b.next_id;
    b.push(-a * (1.0 + ecc) * of_heavy, 0.0, 0.0, -v * of_heavy, heavy, a * 1.0e-5, 0);
    b.push(a * (1.0 + ecc) * of_light, 0.0, 0.0, v * of_light, light, a * 1.0e-5, 0);
    (first, first + 1, (mu / (a * a * a)).sqrt(), v)
}

/// Semi-major axis and eccentricity of the orbit of `j` around `i`, were they alone.
fn orbit(b: &Bodies, i: u32, j: u32) -> (f64, f64) {
    let (i, j) = (b.locate(i).expect("still there"), b.locate(j).expect("still there"));
    let mu = G * (b.m[i] as f64 + b.m[j] as f64);
    let (rx, ry, vx, vy) = (b.x[j] - b.x[i], b.y[j] - b.y[i], b.vx[j] - b.vx[i], b.vy[j] - b.vy[i]);
    let energy = 0.5 * (vx * vx + vy * vy) - mu / rx.hypot(ry);
    let h = rx * vy - ry * vx;
    (-mu / (2.0 * energy), (1.0 + 2.0 * energy * h * h / (mu * mu)).max(0.0).sqrt())
}

/// A hundred revolutions at steps of the world that each cover `turn` radians of them.
/// Returns how far the orbit's size and eccentricity moved, and the drift of the pair as a
/// whole in units of its orbital speed.
fn hundred_turns(b: &mut Bodies, ids: (u32, u32, f64, f64), turn: f64) -> (f64, f64, f64) {
    let (i, j, rate, v) = ids;
    let mut e = Engine::new(G, 0.0);
    let (from, p0) = (orbit(b, i, j), momentum(b));
    for _ in 0..(100.0 * std::f64::consts::TAU / turn).ceil() as usize {
        through(&mut e, b, turn / rate);
    }
    let (to, p) = (orbit(b, i, j), momentum(b));
    (to.0 / from.0 - 1.0, to.1 - from.1, ((p.1 - p0.1) / p.0).hypot((p.2 - p0.2) / p.0) / v)
}

#[test]
fn an_unequal_eccentric_pair_keeps_its_orbit_and_its_momentum() {
    // Sixteen to one, going round five times in a step of the world. Each must take the
    // other's pull at the same moments, or the pair as a whole wanders off (by a tenth of its
    // orbital speed, when each was given steps by the other's mass alone); and each step must
    // be chosen by where the body is going, or the orbit shrinks by a third.
    for ratio in [1.0, 16.0, 1.0e4] {
        let mut b = Bodies::default();
        let ids = pair(&mut b, 1.0e26, ratio, 1.0e8, 0.6);
        let (size, shape, drift) = hundred_turns(&mut b, ids, 30.0);
        assert!(size.abs() < 0.01 && shape.abs() < 0.01, "1:{ratio}: size changed by {size}, eccentricity by {shape}");
        assert!(drift < 1.0e-6, "1:{ratio}: the pair drifted at {drift} of its orbital speed");
    }
}

#[test]
fn a_pair_is_followed_in_a_world_vastly_wider_than_itself() {
    // Light bodies a hundred million times further out than the pair is wide. The world's
    // forces are worked out in single precision, which cannot tell the two apart: they are
    // tied, and what they do to each other is worked out in double precision.
    for others in [2, 300, 6000] {
        let mut b = Bodies::default();
        let ids = pair(&mut b, 1.0e26, 16.0, 1.0e8, 0.0);
        for k in 0..others {
            let t = std::f64::consts::TAU * k as f64 / others as f64;
            let r = 1.0e16 * (0.4 + 0.6 * ((k * 7919) % 1000) as f64 / 1000.0);
            b.push(r * t.cos(), r * t.sin(), 0.0, 0.0, 1.0e10, 1.0, 0);
        }
        let (size, shape, _) = hundred_turns(&mut b, ids, 30.0);
        assert!(size.abs() < 1.0e-3 && shape.abs() < 0.02, "{others} others: size changed by {size}, eccentricity by {shape}");
    }
}

#[test]
fn nothing_depends_on_how_large_or_heavy_things_are() {
    // The same pair a millimetre and a hundred million million kilometres across.
    for (size, mass) in [(1.0e-3, 1.0e-3), (1.0, 1.0e3), (1.0e20, 1.0e37)] {
        let mut b = Bodies::default();
        let ids = pair(&mut b, mass, 16.0, size, 0.6);
        let (changed, shape, drift) = hundred_turns(&mut b, ids, 30.0);
        assert!(changed.abs() < 0.01 && shape.abs() < 0.01 && drift < 1.0e-6, "{size} m, {mass} kg: size changed by {changed}, eccentricity by {shape}, drift {drift}");
    }
}

#[test]
fn own_steps_go_very_deep_and_then_time_slows() {
    // Fifty revolutions in one step of the world are still followed.
    let mut b = Bodies::default();
    let ids = pair(&mut b, 1.0e26, 16.0, 1.0e8, 0.0);
    let (size, shape, _) = hundred_turns(&mut b, ids, 300.0);
    assert!(size.abs() < 1.0e-3 && shape.abs() < 0.02, "size changed by {size}, eccentricity by {shape}");
    // Beyond that the engine says how long a step may be: 65,536 own steps of a tenth of a
    // radian.
    let mut e = Engine::new(G, 0.0);
    through(&mut e, &mut b, 1.0);
    let limit = e.longest_step() * ids.2;
    assert!((6000.0..7000.0).contains(&limit), "a step may cover {limit} radians of the tightest orbit");
    e.own_steps = false;
    assert_eq!(e.longest_step(), f64::MAX);

    // And the runner keeps to it, or to what the ties cost before that: slowed, but not to
    // the step a world stepped whole would need.
    let mut b = small("cloud", 2000.0).bodies;
    let (m, d) = (1.0e30, 5.0e8);
    let v = (G * m / (2.0 * d)).sqrt();
    b.push(1.0e12 - 0.5 * d, 0.0, 0.0, -v, m, 5.0e7, 0);
    b.push(1.0e12 + 0.5 * d, 0.0, 0.0, v, m, 5.0e7, 0);
    let rate = (G * 2.0 * m / d.powi(3)).sqrt();
    let runner = Runner::start(Sim::new(scenario::Setup { bodies: b, ..small("cloud", 2000.0) }), 86_400.0 * 3.0e4, 4);
    std::thread::sleep(Duration::from_millis(2500));
    let p = runner.published.lock().unwrap();
    assert!(p.stats.limited, "a step of {} s went unquestioned", p.stats.dt);
    assert!(p.stats.dt * rate < 7200.0 && p.stats.dt * rate > 1.0, "steps of {} radians of the tightest orbit", p.stats.dt * rate);
}
