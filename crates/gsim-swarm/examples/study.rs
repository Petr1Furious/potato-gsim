//! Measurements of how well the engine follows orbits and encounters it cannot resolve with
//! the world's step alone, on small constructed worlds whose right answer is known, and of
//! what that costs in crowded ones.
//!
//! cargo run --release -p gsim-swarm --example study [binary|moon|flyby|width|scale|energy|cost]...

use gsim_swarm::scenario::{self, Params};
use gsim_swarm::{Bodies, Engine, Sim};
use std::f64::consts::TAU;

const G: f64 = 6.67430e-11;

/// One step through the phases a large world goes through, whatever the number of bodies.
fn step(e: &mut Engine, b: &mut Bodies, dt: f64) -> usize {
    e.begin(b, dt);
    while e.own(b, std::time::Duration::MAX) {}
    e.end(b);
    e.forces(b);
    e.finish(b).len()
}

/// The longest step the engine would take by itself, given what it found last step.
fn longest(e: &Engine) -> f64 {
    e.longest_step()
}

#[derive(Clone, Copy)]
struct Orbit {
    /// Semi-major axis and eccentricity.
    a: f64,
    e: f64,
    /// Direction from the first body to the second.
    angle: f64,
    /// Energy per unit of reduced mass.
    energy: f64,
}

/// The orbit of body `j` around body `i` (by id), had they been alone.
fn orbit(b: &Bodies, i: u32, j: u32) -> Option<Orbit> {
    let (i, j) = (b.locate(i)?, b.locate(j)?);
    let mu = G * (b.m[i] as f64 + b.m[j] as f64);
    let (rx, ry, vx, vy) = (b.x[j] - b.x[i], b.y[j] - b.y[i], b.vx[j] - b.vx[i], b.vy[j] - b.vy[i]);
    let r = rx.hypot(ry);
    let energy = 0.5 * (vx * vx + vy * vy) - mu / r;
    let h = rx * vy - ry * vx;
    Some(Orbit { a: -mu / (2.0 * energy), e: (1.0 + 2.0 * energy * h * h / (mu * mu)).max(0.0).sqrt(), angle: ry.atan2(rx), energy })
}

/// Velocity of the centre of mass of everything.
fn drift(b: &Bodies) -> (f64, f64) {
    let (mut m, mut px, mut py) = (0.0, 0.0, 0.0);
    for i in 0..b.len() {
        let w = b.m[i] as f64;
        (m, px, py) = (m + w, px + w * b.vx[i], py + w * b.vy[i]);
    }
    (px / m, py / m)
}

/// Two bodies on an orbit with the given mass ratio and eccentricity, starting furthest apart.
/// Returns the ids, the rate (rad/s) of the mean motion and the relative speed at the start.
fn pair(b: &mut Bodies, at: (f64, f64), heavy: f64, ratio: f64, a: f64, ecc: f64) -> (u32, u32, f64, f64) {
    let light = heavy / ratio;
    let mu = G * (heavy + light);
    let apo = a * (1.0 + ecc);
    let v = (mu * (1.0 - ecc) / (a * (1.0 + ecc))).sqrt();
    let (fh, fl) = (light / (heavy + light), heavy / (heavy + light));
    let first = b.next_id;
    b.push(at.0 - apo * fh, at.1, 0.0, -v * fh, heavy, a * 1.0e-5, 0);
    b.push(at.0 + apo * fl, at.1, 0.0, v * fl, light, a * 1.0e-5, 0);
    (first, first + 1, (mu / (a * a * a)).sqrt(), v)
}

fn wrapped(a: f64) -> f64 {
    (a + TAU / 2.0).rem_euclid(TAU) - TAU / 2.0
}

/// A pair by itself: how its orbit holds up when the world's step is `turn` radians of it.
fn binary() {
    println!("\n== a pair by itself, 100 revolutions ==");
    println!("{:>9} {:>4} {:>7} | {:>9} {:>9} {:>9} {:>9} {:>6}", "ratio", "ecc", "turn", "da/a", "de", "phase", "drift/v", "most");
    for ratio in [1.0, 16.0, 1.0e3, 1.0e6] {
        for ecc in [0.0, 0.6] {
            for turn in [0.05, 1.0, 30.0, 300.0, 3000.0] {
                let mut b = Bodies::default();
                let (i, j, rate, v) = pair(&mut b, (0.0, 0.0), 1.0e26, ratio, 1.0e8, ecc);
                let mut e = Engine::new(G, 0.0);
                let dt = turn / rate;
                let steps = ((100.0 * TAU / turn).ceil() as usize).max(40);
                let (from, mut most) = (orbit(&b, i, j).unwrap(), 0);
                let mut left = true;
                for _ in 0..steps {
                    step(&mut e, &mut b, dt);
                    most = most.max(e.stats.most);
                    left &= b.len() == 2;
                }
                let Some(to) = orbit(&b, i, j).filter(|_| left) else {
                    println!("{ratio:>9.0e} {ecc:>4} {turn:>7} | merged");
                    continue;
                };
                let d = drift(&b);
                // Where the pair should point after that long, for a round orbit.
                let phase = if ecc == 0.0 { format!("{:>9.2e}", wrapped(to.angle - from.angle - rate * dt * steps as f64).abs()) } else { format!("{:>9}", "") };
                println!("{ratio:>9.0e} {ecc:>4} {turn:>7} | {:>9.2e} {:>9.2e} {phase} {:>9.2e} {most:>6}", to.a / from.a - 1.0, to.e - from.e, d.0.hypot(d.1) / v);
            }
        }
    }
}

/// A star, a planet and a moon: the moon's orbit over one year of the planet.
fn moon() {
    println!("\n== star, planet, moon: one revolution of the planet ==");
    println!("{:>7} | {:>10} {:>10} {:>10} {:>10} {:>6}", "turn", "moon da/a", "moon ecc", "planet da/a", "drift/v", "most");
    for turn in [0.05, 1.0, 30.0, 300.0, 3000.0] {
        let mut b = Bodies::default();
        let (star, planet, moon) = (1.0e30, 1.0e26, 1.0e21);
        let (far, near) = (1.0e11, 1.0e8);
        let (v_planet, v_moon) = ((G * star / far).sqrt(), (G * planet / near).sqrt());
        b.push(0.0, 0.0, 0.0, 0.0, star, 1.0e6, 0);
        b.push(far, 0.0, 0.0, v_planet, planet, 1.0e5, 0);
        b.push(far + near, 0.0, 0.0, v_planet + v_moon, moon, 1.0e4, 0);
        let rate = (G * planet / near.powi(3)).sqrt();
        let year = TAU / (G * star / far.powi(3)).sqrt();
        let dt = turn / rate;
        let mut e = Engine::new(G, 0.0);
        let (m0, p0, mut most) = (orbit(&b, 1, 2).unwrap(), orbit(&b, 0, 1).unwrap(), 0);
        for _ in 0..(year / dt).ceil() as usize {
            step(&mut e, &mut b, dt);
            most = most.max(e.stats.most);
        }
        let d = drift(&b);
        match (orbit(&b, 1, 2), orbit(&b, 0, 1)) {
            (Some(m), Some(p)) => println!("{turn:>7} | {:>10.2e} {:>10.2e} {:>10.2e} {:>10.2e} {most:>6}", m.a / m0.a - 1.0, m.e, p.a / p0.a - 1.0, d.0.hypot(d.1) / v_moon),
            _ => println!("{turn:>7} | merged"),
        }
    }
}

/// A moon far out, where the star's pull across its orbit matters: how far it strays from
/// its planet over thirty of the planet's years, next to the same world stepped whole in
/// double precision with short steps. `reach` is the moon's distance in units of the
/// furthest at which the planet can hold anything at all.
fn tide() {
    println!("\n== a moon far from its planet, thirty revolutions of the planet ==");
    println!("{:>6} {:>7} | {:>16} {:>16} {:>10} {:>6}", "reach", "turn", "moon from..to", "exact from..to", "planet da/a", "most");
    let (star, planet, moon, far) = (1.0e30f64, 1.0e26f64, 1.0e21f64, 1.0e11f64);
    let hill = far * (planet / (3.0 * star)).cbrt();
    for reach in [0.05, 0.1, 0.2] {
        let near = reach * hill;
        let world = || {
            let mut b = Bodies::default();
            let (v_planet, v_moon) = ((G * star / far).sqrt(), (G * planet / near).sqrt());
            b.push(0.0, 0.0, 0.0, 0.0, star, 1.0e6, 0);
            b.push(far, 0.0, 0.0, v_planet, planet, 1.0e5, 0);
            b.push(far + near, 0.0, 0.0, v_planet + v_moon, moon, 1.0e4, 0);
            b
        };
        let rate = (G * planet / near.powi(3)).sqrt();
        let years = 30.0 * TAU / (G * star / far.powi(3)).sqrt();
        let apart = |b: &Bodies| b.locate(1).zip(b.locate(2)).map_or(f64::NAN, |(p, m)| (b.x[m] - b.x[p]).hypot(b.y[m] - b.y[p]) / near);
        // The same world stepped whole: few enough bodies for that.
        let mut exact = (f64::MAX, 0.0f64);
        let (mut b, mut e) = (world(), Engine::new(G, 0.0));
        for _ in 0..(years * rate / 0.02) as usize {
            e.step(&mut b, 0.02 / rate);
            exact = (exact.0.min(apart(&b)), exact.1.max(apart(&b)));
        }
        for turn in [0.5, 1.0, std::f64::consts::PI, 5.0, TAU, 30.0] {
            let (mut b, mut e) = (world(), Engine::new(G, 0.0));
            let (p0, dt) = (orbit(&b, 0, 1).unwrap(), turn / rate);
            let (mut seen, mut most) = ((f64::MAX, 0.0f64), 0);
            for _ in 0..(years / dt).ceil() as usize {
                step(&mut e, &mut b, dt);
                most = most.max(e.stats.most);
                seen = (seen.0.min(apart(&b)), seen.1.max(apart(&b)));
            }
            let planet = orbit(&b, 0, 1).map_or(f64::NAN, |p| p.a / p0.a - 1.0);
            println!("{reach:>6} {turn:>7} | {:>7.3}..{:<7.3} {:>7.3}..{:<7.3} {planet:>10.2e} {most:>6}", seen.0, seen.1, exact.0, exact.1);
        }
    }
}

/// Two equal bodies passing each other once: how far each is turned, against the exact
/// answer, and whether they come out as fast as they went in.
fn flyby() {
    println!("\n== two bodies passing once, 16 starting moments each ==");
    println!("{:>6} {:>9} | {:>9} {:>9} {:>9} {:>6} {:>6}", "speed", "reach", "turn err", "worst", "energy", "bound", "merged");
    let (mass, miss) = (1.0e24, 1.0e7);
    for speed in [1.5, 4.0, 20.0] {
        // Travelled in one step of the world, in units of the closest distance.
        for reach in [0.03, 1.0, 30.0, 1000.0] {
            let mu = G * 2.0 * mass;
            let v = speed * (mu / miss).sqrt();
            let dt = reach * miss / v;
            let exact = 2.0 * (mu / (miss * v * v)).atan();
            let start = (300.0 * miss).max(8.0 * v * dt);
            let (mut sum, mut worst, mut gain, mut bound, mut merged) = (0.0f64, 0.0f64, 0.0f64, 0, 0);
            for k in 0..16 {
                let mut b = Bodies::default();
                let x = start + v * dt * k as f64 / 16.0;
                b.push(-0.5 * x, -0.5 * miss, 0.5 * v, 0.0, mass, miss * 1.0e-4, 0);
                b.push(0.5 * x, 0.5 * miss, -0.5 * v, 0.0, mass, miss * 1.0e-4, 0);
                let mut e = Engine::new(G, 0.0);
                for _ in 0..(2.0 * x / (v * dt)).ceil() as usize + 4 {
                    step(&mut e, &mut b, dt);
                }
                let (Some(i), Some(j)) = (b.locate(0), b.locate(1)) else {
                    merged += 1;
                    continue;
                };
                let (ux, uy) = (b.vx[i] - b.vx[j], b.vy[i] - b.vy[j]);
                let to = orbit(&b, 0, 1).unwrap();
                bound += (to.energy < 0.0) as u32;
                // Energy at infinity is all speed.
                gain = gain.max((to.energy / (0.5 * v * v) - 1.0).abs());
                let off = (wrapped(uy.atan2(ux)).abs() - exact).abs() / exact;
                (sum, worst) = (sum + off, worst.max(off));
            }
            let n = (16 - merged).max(1) as f64;
            println!("{speed:>6} {reach:>9} | {:>9.2e} {worst:>9.2e} {gain:>9.2e} {bound:>6} {merged:>6}", sum / n);
        }
    }
}

/// A pair in a world much wider than itself: far-off bodies stretch what the engine measures in.
fn width() {
    println!("\n== a pair (ratio 16) with bystanders far away, 100 revolutions ==");
    println!("{:>6} {:>9} {:>7} | {:>9} {:>9} {:>9}", "turn", "width", "others", "da/a", "de", "phase");
    // A step so short that the pair needs none of its own, and one that takes five turns.
    for (turn, others) in [(0.02, 2), (0.02, 200), (0.02, 6000), (30.0, 2), (30.0, 6000)] {
        for wide in [1.0e2, 1.0e4, 1.0e6, 1.0e8, 1.0e10] {
            let mut b = Bodies::default();
            let a = 1.0e8;
            let (i, j, rate, _) = pair(&mut b, (0.0, 0.0), 1.0e26, 16.0, a, 0.0);
            // Light, slow, and all around.
            let mut dice = 0x9E37_79B9_7F4A_7C15u64;
            let mut random = || {
                dice ^= dice << 13;
                dice ^= dice >> 7;
                dice ^= dice << 17;
                (dice >> 11) as f64 / (1u64 << 53) as f64
            };
            for k in 0..others {
                let (r, t) = (a * wide * (0.3 + 0.7 * random()), TAU * (k as f64 + random()) / others as f64);
                b.push(r * t.cos(), r * t.sin(), 0.0, 0.0, 1.0e10, 1.0, 0);
            }
            let mut e = Engine::new(G, 0.0);
            let dt = turn / rate;
            let steps = (100.0 * TAU / turn).ceil() as usize;
            let from = orbit(&b, i, j).unwrap();
            for _ in 0..steps {
                step(&mut e, &mut b, dt);
            }
            match orbit(&b, i, j) {
                Some(to) => println!("{turn:>6} {wide:>9.0e} {others:>7} | {:>9.2e} {:>9.2e} {:>9.2e}", to.a / from.a - 1.0, to.e - from.e, wrapped(to.angle - from.angle - rate * dt * steps as f64).abs()),
                None => println!("{turn:>6} {wide:>9.0e} {others:>7} | merged"),
            }
        }
    }
}

/// The same pair at very different sizes and masses: nothing should depend on either.
fn scale() {
    println!("\n== the same pair (ratio 16, ecc 0.6, turn 30) at different scales, 100 revolutions ==");
    println!("{:>9} {:>9} | {:>9} {:>9} {:>9}", "size m", "mass kg", "da/a", "de", "drift/v");
    for (size, mass) in [(1.0e-3, 1.0e-3), (1.0, 1.0e3), (1.0e3, 1.0e12), (1.0e8, 1.0e26), (1.0e14, 1.0e33), (1.0e20, 1.0e37)] {
        let mut b = Bodies::default();
        let (i, j, rate, v) = pair(&mut b, (0.0, 0.0), mass, 16.0, size, 0.6);
        let mut e = Engine::new(G, 0.0);
        let dt = 30.0 / rate;
        let from = orbit(&b, i, j).unwrap();
        for _ in 0..(100.0 * TAU / 30.0f64).ceil() as usize {
            step(&mut e, &mut b, dt);
        }
        let d = drift(&b);
        match orbit(&b, i, j) {
            Some(to) => println!("{size:>9.0e} {mass:>9.0e} | {:>9.2e} {:>9.2e} {:>9.2e}", to.a / from.a - 1.0, to.e - from.e, d.0.hypot(d.1) / v),
            None => println!("{size:>9.0e} {mass:>9.0e} | merged"),
        }
    }
}

fn params(count: f64) -> Params {
    Params::from([("count".to_string(), count)])
}

/// Kinetic plus potential energy, summed over every pair.
fn total_energy(b: &Bodies) -> f64 {
    use rayon::prelude::*;
    (0..b.len())
        .into_par_iter()
        .map(|i| {
            let mi = b.m[i] as f64;
            let mut e = 0.5 * mi * (b.vx[i] * b.vx[i] + b.vy[i] * b.vy[i]);
            for j in 0..i {
                e -= G * mi * b.m[j] as f64 / (b.x[i] - b.x[j]).hypot(b.y[i] - b.y[j]).max(1.0e-300);
            }
            e
        })
        .sum()
}

/// A crowd left to itself: how well energy and momentum are kept.
fn energy() {
    println!("\n== crowds: energy and momentum over 300 steps ==");
    println!("{:>10} {:>7} {:>5} | {:>9} {:>9} {:>7} {:>7} {:>6}", "world", "bodies", "own", "dE/E", "drift/v", "merged", "fine", "most");
    for (name, count) in [("cloud", 3000.0), ("cloud", 12_000.0), ("collision", 12_000.0), ("galaxy", 12_000.0)] {
        for own in [false, true] {
            let setup = scenario::build(name, 7, &params(count)).unwrap();
            let dt = setup.time_scale * 0.01;
            let mut b = setup.bodies;
            let mut e = Engine::new(G, 0.0);
            e.theta = setup.theta;
            e.own_steps = own;
            let (e0, d0) = (total_energy(&b), drift(&b));
            let speed = (0..b.len()).map(|i| b.vx[i].hypot(b.vy[i])).sum::<f64>() / b.len() as f64;
            let (mut merged, mut fine, mut most) = (0, 0, 0);
            for _ in 0..300 {
                let dt = dt.min(longest(&e));
                merged += step(&mut e, &mut b, dt);
                (fine, most) = (fine.max(e.stats.fine), most.max(e.stats.most));
            }
            let d = drift(&b);
            println!("{name:>10} {count:>7} {own:>5} | {:>9.2e} {:>9.2e} {merged:>7} {fine:>7} {most:>6}", total_energy(&b) / e0 - 1.0, (d.0 - d0.0).hypot(d.1 - d0.1) / speed);
        }
    }
}

/// Crowded worlds run the way the game runs them, for a minute of the game's time: how much
/// extra work bodies on steps of their own are, and how much time has to be slowed for them.
///
/// Work is counted in forces evaluated, not in time, so that it does not depend on what else
/// the machine is doing: `step_ms` is what a step without own steps takes on a quiet machine.
fn cost() {
    println!("\n== crowded worlds at the pace they start with, 60 s of play ==");
    println!("{:>10} {:>7} | {:>6} {:>7} {:>8} {:>6} {:>6} | {:>6} {:>6}", "world", "bodies", "fine", "most", "work", "pace", "slowed", "merged", "left");
    for (name, count, step_ms) in [("collision", 50_000.0, 2.7), ("collision", 150_000.0, 7.7), ("galaxy", 150_000.0, 9.2), ("cloud", 150_000.0, 3.3), ("galaxy", 400_000.0, 25.0)] {
        let setup = scenario::build(name, 7, &params(count)).unwrap();
        let pace = setup.time_scale;
        let mut sim = Sim::new(setup);
        let wanted = pace * step_ms * 1.0e-3;
        let (mut steps, mut fine, mut most, mut full, mut own, mut merged) = (0u64, 0u64, 0u32, 0.0f64, 0.0f64, 0usize);
        // Seconds of play: a step takes longer by the work its own steps are.
        let (mut played, mut slowed) = (0.0f64, 0.0f64);
        while played < 60.0 {
            let dt = wanted.min(longest(&sim.engine));
            merged += sim.step(dt).len();
            let s = sim.engine.stats;
            let took = step_ms * 1.0e-3 * (1.0 + (s.own_interactions / s.interactions.max(1.0)) as f64);
            (steps, fine, most) = (steps + 1, fine + s.fine as u64, most.max(s.most));
            (full, own) = (full + s.interactions as f64, own + s.own_interactions as f64);
            (played, slowed) = (played + took, slowed + took * (1.0 - dt / wanted));
        }
        println!(
            "{name:>10} {count:>7} | {:>6} {most:>7} {:>7.0}% {:>5.0}% {:>5.0}% | {merged:>6} {:>6}",
            fine / steps,
            100.0 * own / full,
            100.0 * sim.time / (pace * played),
            100.0 * slowed / played,
            sim.bodies.read().unwrap().len()
        );
    }
}

/// Crowded worlds run the way the game runs them, by the clock: how much of the pace asked for
/// is held. Only means something on a machine that is doing nothing else.
fn pace() {
    played(false);
}

/// The same with the worlds as they were set up before they were slowed down and their bodies
/// made larger: twice the pace, and smaller bodies in the cloud and the collision.
fn harder() {
    played(true);
}

fn played(harder: bool) {
    println!("\n== crowded worlds by the clock, 10 s each{} ==", if harder { ", at twice the pace and with smaller bodies" } else { "" });
    println!("{:>10} {:>7} {:>5} | {:>6} {:>8} {:>8} {:>8} {:>6} {:>6} {:>6}", "world", "bodies", "own", "pace", "steps/s", "own ms", "rest ms", "fine", "most", "slowed");
    for (name, count) in [("collision", 50_000.0), ("collision", 150_000.0), ("galaxy", 150_000.0), ("cloud", 150_000.0), ("galaxy", 400_000.0)] {
        for own in [false, true] {
            let mut params = params(count);
            if harder {
                let (pace, size) = match name {
                    "collision" => (1_728_000.0, 0.03),
                    "cloud" => (86_400.0, 0.3),
                    _ => (86_400.0, 0.1),
                };
                params.extend([("time_scale".to_string(), pace), ("size".to_string(), size)]);
            }
            let setup = scenario::build(name, 7, &params).unwrap();
            let pace = setup.time_scale;
            let mut sim = Sim::new(setup);
            sim.engine.own_steps = own;
            // As the runner does it: a step covers what the last one took, at the pace asked for.
            let (mut base_s, mut steps, mut held, mut most) = (0.002f64, 0u32, 0u32, 0u32);
            let (mut own_ms, mut rest_ms, mut fine) = (0.0f64, 0.0f64, 0u64);
            let started = std::time::Instant::now();
            let from = sim.time;
            while started.elapsed().as_secs_f64() < 10.0 {
                let (wanted, limit) = (pace * base_s.min(0.1), longest(&sim.engine));
                let t0 = std::time::Instant::now();
                sim.step(wanted.min(limit));
                let took = t0.elapsed().as_secs_f64();
                let s = sim.engine.stats;
                base_s += 0.15 * ((took - s.own_ms as f64 * 1.0e-3).max(0.0) - base_s);
                (steps, held, most) = (steps + 1, held + (limit < wanted) as u32, most.max(s.most));
                (own_ms, rest_ms, fine) = (own_ms + s.own_ms as f64, rest_ms + took * 1.0e3 - s.own_ms as f64, fine + s.fine as u64);
            }
            let spent = started.elapsed().as_secs_f64();
            let n = steps as f64;
            println!(
                "{name:>10} {count:>7} {own:>5} | {:>5.0}% {:>8.1} {:>8.1} {:>8.1} {:>6} {most:>6} {:>5.0}%",
                100.0 * (sim.time - from) / (pace * spent),
                n / spent,
                own_ms / n,
                rest_ms / n,
                fine / steps as u64,
                100.0 * held as f64 / n
            );
        }
    }
}

fn main() {
    let asked: Vec<String> = std::env::args().skip(1).collect();
    let all: [(&str, fn()); 10] = [("binary", binary), ("moon", moon), ("flyby", flyby), ("width", width), ("scale", scale), ("energy", energy), ("cost", cost), ("pace", pace), ("tide", tide), ("harder", harder)];
    for (name, run) in all {
        if asked.is_empty() || asked.iter().any(|a| a == name) {
            run();
        }
    }
}
