//! A world that can be stepped and edited: the engine plus the bodies it works on, shared
//! with whoever draws them.

use crate::engine::{Bodies, Engine, Merge};
use crate::scenario::Setup;
use crate::Level;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const G: f64 = 6.67430e-11;
/// Plummer softening between bodies (m).
const SOFTENING: f64 = 1.0e6;

pub struct Sim {
    pub engine: Engine,
    /// Shared with whoever draws the world; only written inside the methods here.
    pub bodies: Arc<RwLock<Bodies>>,
    /// Simulated seconds since the start.
    pub time: f64,
    pub steps: u64,
    /// Bodies (by id) to keep following through merges: the selected one, typically.
    pub watch: Vec<u32>,
    pub merges_total: u64,
    pub removed_total: u64,
    seed: u64,
    /// Raised by whoever wants to read the bodies. A small world is stepped hundreds of
    /// thousands of times a second, each step taking the lock for itself; without this a
    /// reader could wait a long time for a gap.
    pub reader_waiting: Arc<AtomicBool>,
}

impl Sim {
    pub fn new(setup: Setup) -> Self {
        Self::with_level(setup, Level::detect())
    }

    pub fn with_level(setup: Setup, level: Level) -> Self {
        let mut engine = Engine::with_level(level, G, SOFTENING);
        engine.theta = setup.theta;
        let mut bodies = setup.bodies;
        engine.prime(&mut bodies);
        Self { engine, bodies: Arc::new(RwLock::new(bodies)), time: 0.0, steps: 0, watch: Vec::new(), merges_total: 0, removed_total: 0, seed: 0x9E37_79B9, reader_waiting: Arc::new(AtomicBool::new(false)) }
    }

    /// Stand aside while somebody is waiting to read the bodies (but not forever: a reader
    /// that never lowers its flag must not stop the world).
    pub fn let_readers_in(&self) {
        let since = Instant::now();
        while self.reader_waiting.load(Ordering::Acquire) && since.elapsed() < Duration::from_millis(50) {
            std::thread::yield_now();
        }
    }

    /// Few enough bodies that a step is done on the calling thread alone.
    pub fn small(&self) -> bool {
        self.engine.precise(&self.bodies.read().unwrap())
    }

    /// Advance the world by `dt` seconds. The long middle part only reads the bodies, so
    /// they can be drawn meanwhile.
    pub fn step(&mut self, dt: f64) -> Vec<Merge> {
        let small = self.engine.precise(&self.bodies.read().unwrap());
        self.let_readers_in();
        let merges = if small {
            let mut b = self.bodies.write().unwrap();
            let merges = self.engine.step(&mut b, dt);
            (b.time, b.dt) = (self.time + dt, dt);
            self.engine.leave(&mut b);
            merges
        } else {
            {
                let mut b = self.bodies.write().unwrap();
                self.engine.advance(&mut b, dt);
                (b.time, b.dt) = (self.time + dt, dt);
            }
            {
                let b = self.bodies.read().unwrap();
                self.engine.forces(&b);
            }
            self.let_readers_in();
            let mut b = self.bodies.write().unwrap();
            let merges = self.engine.finish(&mut b, dt);
            self.engine.leave(&mut b);
            merges
        };
        for m in &merges {
            for w in self.watch.iter_mut().filter(|w| **w == m.absorbed) {
                *w = m.survivor;
            }
        }
        self.merges_total += merges.len() as u64;
        self.removed_total += self.engine.stats.removed as u64;
        self.time += dt;
        self.steps += 1;
        merges
    }

    /// Change the bodies from outside a step; everything derived from them is redone.
    fn change<T>(&mut self, edit: impl FnOnce(&mut Bodies) -> T) -> T {
        self.let_readers_in();
        let mut b = self.bodies.write().unwrap();
        let out = edit(&mut b);
        self.engine.invalidate();
        self.engine.prime(&mut b);
        out
    }

    /// Work everything out afresh from the bodies as they are (after changing what the
    /// engine should look for, say).
    pub fn refresh(&mut self) {
        self.change(|_| ());
    }

    /// Add bodies; they get new ids.
    pub fn add(&mut self, new: &Bodies) {
        self.change(|b| b.append(new));
    }

    /// Remove everything within `r` of a point. Returns how many bodies went.
    pub fn erase(&mut self, x: f64, y: f64, r: f64) -> usize {
        self.change(|b| {
            let before = b.len();
            b.retain(|b, i| (b.x[i] - x).powi(2) + (b.y[i] - y).powi(2) > r * r);
            before - b.len()
        })
    }

    pub fn remove(&mut self, id: u32) -> bool {
        self.change(|b| {
            let before = b.len();
            b.retain(|b, i| b.id[i] != id);
            b.len() < before
        })
    }

    /// Change a body's mass, radius and velocity. False if it is gone.
    pub fn edit(&mut self, id: u32, mass: f64, radius: f64, vx: f64, vy: f64) -> bool {
        self.change(|b| {
            let Some(i) = b.locate(id) else { return false };
            (b.m[i], b.r[i], b.vx[i], b.vy[i]) = (mass.max(1.0) as f32, radius.max(1.0) as f32, vx, vy);
            true
        })
    }

    fn random(&mut self) -> f64 {
        self.seed = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Break a body into `pieces` equal fragments flying apart. `violence` is their speed
    /// at the edge as a multiple of the body's escape speed. False if the body is gone.
    pub fn shatter(&mut self, id: u32, pieces: usize, violence: f64) -> bool {
        let pieces = pieces.clamp(2, 5000);
        let turn = self.random() * std::f64::consts::TAU;
        self.change(|b| {
            let Some(i) = b.locate(id) else { return false };
            let (x, y, vx, vy, mass, radius, group) = (b.x[i], b.y[i], b.vx[i], b.vy[i], b.m[i] as f64, b.r[i] as f64, b.group[i]);
            b.retain(|b, k| b.id[k] != id);
            // Same total volume; laid out on a sunflower spiral with room between them, so
            // they do not fall straight back into one another.
            let each = radius / (pieces as f64).cbrt();
            let pitch = 1.6 * each;
            let edge = pitch * (pieces as f64).sqrt();
            let escape = (2.0 * G * mass / radius).sqrt();
            for k in 0..pieces {
                let (r, a) = (pitch * (k as f64 + 0.5).sqrt(), turn + k as f64 * 2.399_963_229_728_653);
                let speed = violence * escape * r / edge;
                b.push(x + r * a.cos(), y + r * a.sin(), vx + speed * a.cos(), vy + speed * a.sin(), mass / pieces as f64, each, group);
            }
            true
        })
    }

    /// Swap the whole world for another (a saved one, or a checkpoint).
    pub fn replace(&mut self, bodies: Bodies) {
        self.time = bodies.time;
        self.change(|b| *b = bodies);
    }
}
