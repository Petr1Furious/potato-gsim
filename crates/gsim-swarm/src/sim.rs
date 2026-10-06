//! A large world with one ship in it. The ship is a `gsim-core` test particle integrated
//! against the few hundred bodies and lumps that matter at its position.

use crate::engine::{Bodies, Engine, Local, Merge};
use crate::scenario::Setup;
use crate::Level;
use gsim_core::ship::step_ship;
use gsim_core::{GameRules, MassiveSnapshot, MassiveView, Particle, Scratch, ShipInput, ShipState};
use std::sync::{Arc, RwLock};

/// How finely the world is resolved around the ship (opening angle of its own tree walk).
const SHIP_THETA: f64 = 0.85;
/// The ship's neighbourhood is resized to keep its list of attractors within these bounds.
const LOCAL_MIN: usize = 200;
const LOCAL_MAX: usize = 450;
/// Acceleration of a ship at full thrust (m/s^2), as in `GameRules`.
const THRUST: f64 = 0.02;

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Spawned,
    /// The ship ran into the body with this id.
    Crashed { x: f64, y: f64, vx: f64, vy: f64, body: u32 },
}

pub struct Sim {
    pub rules: GameRules,
    pub engine: Engine,
    /// Shared with whoever draws the world; only written inside [`Sim::step`].
    pub bodies: Arc<RwLock<Bodies>>,
    pub tick: u64,
    /// Simulated seconds since the start.
    pub time: f64,
    pub ship: Option<ShipState>,
    /// The ship one tick ago (for drawing between ticks).
    pub ship_before: Option<ShipState>,
    pub respawn_tick: Option<u64>,
    /// Nothing can destroy the ship and its fuel never runs out.
    pub god: bool,
    /// Bodies (by id) to keep following through merges and to resolve individually around
    /// the ship: the selected one, typically.
    pub watch: Vec<u32>,
    pub merges_total: u64,
    pub removed_total: u64,
    pub deaths: u32,
    /// What acted on the ship at `local_tick`.
    pub local: Local,
    pub local_tick: u64,
    spawn_r: (f64, f64),
    spawn_around: Option<u32>,
    near: f64,
    seed: u64,
    /// Fraction of a mm/s of thrust not yet spent (steps can be too short for a whole one).
    burn_carry: f64,
    scratch: Scratch,
    no_radius: Vec<f64>,
    alive: Vec<bool>,
}

impl Sim {
    pub fn new(setup: Setup, seed: u64) -> Self {
        Self::with_level(setup, seed, Level::detect())
    }

    pub fn with_level(setup: Setup, seed: u64, level: Level) -> Self {
        let rules = GameRules::new(setup.time_scale, 60);
        let mut engine = Engine::with_level(level, rules.g, rules.softening);
        engine.theta = setup.theta;
        engine.escape_radius = setup.escape_radius;
        let mut bodies = setup.bodies;
        engine.prime(&mut bodies);
        let near = engine.extent() / 40.0;
        Self {
            rules,
            engine,
            bodies: Arc::new(RwLock::new(bodies)),
            tick: 0,
            time: 0.0,
            burn_carry: 0.0,
            ship: None,
            ship_before: None,
            respawn_tick: Some(0),
            god: false,
            watch: Vec::new(),
            merges_total: 0,
            removed_total: 0,
            deaths: 0,
            local: Local::default(),
            local_tick: 0,
            spawn_r: setup.spawn_r,
            spawn_around: setup.spawn_around,
            near,
            seed,
            scratch: Scratch::default(),
            no_radius: Vec::new(),
            alive: Vec::new(),
        }
    }

    /// Change how much simulated time a step covers, as simulated seconds per real second.
    /// Any positive value goes: long steps are fast and crude, short ones slow and exact.
    pub fn set_pace(&mut self, pace: f64) {
        if pace.is_finite() && pace > 0.0 {
            self.rules = GameRules::new(pace, self.rules.tick_hz);
        }
    }

    fn random(&mut self) -> f64 {
        self.seed = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Position, velocity and acceleration of what ships orbit when they appear.
    fn anchor(&self, b: &Bodies) -> [f64; 6] {
        match self.spawn_around.and_then(|id| b.locate(id)) {
            Some(i) => [b.x[i], b.y[i], b.vx[i], b.vy[i], b.ax[i] as f64, b.ay[i] as f64],
            None => {
                let (x, y, vx, vy) = self.engine.barycentre();
                [x, y, vx, vy, 0.0, 0.0]
            }
        }
    }

    /// A state on a circular orbit about `centre` at `(x, y)`, judged from the pull actually
    /// felt there. `None` if the spot is inside or right next to a body.
    fn orbit_at(&self, b: &Bodies, x: f64, y: f64, centre: [f64; 6]) -> Option<Particle> {
        let mut local = Local::default();
        self.engine.local(b, x, y, SHIP_THETA, self.near, &[], &mut local);
        let crowded = (0..local.len()).any(|j| {
            let (dx, dy) = (local.x[j] - x, local.y[j] - y);
            dx * dx + dy * dy < (6.0 * local.radius[j]).powi(2)
        });
        if crowded {
            return None;
        }
        let (ax, ay) = local.accel_at(x, y, self.rules.g, self.rules.softening);
        let (rx, ry) = (x - centre[0], y - centre[1]);
        let d = (rx * rx + ry * ry).sqrt().max(1.0);
        // Pull towards the centre, in the centre's own (accelerating) frame.
        let inward = -((ax - centre[4]) * rx + (ay - centre[5]) * ry) / d;
        let v = (inward.max(0.0) * d).sqrt();
        Some(Particle { x, y, vx: centre[2] - v * ry / d, vy: centre[3] + v * rx / d })
    }

    fn spawn(&mut self) -> bool {
        let bodies = self.bodies.clone();
        let b = bodies.read().unwrap();
        let centre = self.anchor(&b);
        for _ in 0..16 {
            let r = self.spawn_r.0 + (self.spawn_r.1 - self.spawn_r.0) * self.random();
            let a = self.random() * std::f64::consts::TAU;
            if let Some(p) = self.orbit_at(&b, centre[0] + r * a.cos(), centre[1] + r * a.sin(), centre) {
                self.place(p);
                return true;
            }
        }
        false
    }

    /// Put the ship somewhere with full tanks.
    pub fn place(&mut self, p: Particle) {
        let fuel = self.ship.map_or(self.rules.fuel_max_mmps, |s| s.fuel);
        self.ship = Some(ShipState { p, fuel, idle_ticks: 0 });
        self.ship_before = self.ship;
        self.respawn_tick = None;
    }

    /// Circular orbit around a body, ten radii out. False if the body is gone.
    pub fn orbit(&mut self, id: u32) -> bool {
        let bodies = self.bodies.clone();
        let b = bodies.read().unwrap();
        let Some(i) = b.locate(id) else { return false };
        let r = 10.0 * b.r[i] as f64;
        let v = (self.rules.g * b.m[i] as f64 / r).sqrt();
        self.place(Particle { x: b.x[i] + r, y: b.y[i], vx: b.vx[i], vy: b.vy[i] + v });
        true
    }

    pub fn refuel(&mut self) {
        if let Some(s) = self.ship.as_mut() {
            s.fuel = self.rules.fuel_max_mmps;
        }
    }

    /// Destroy the ship; a new one appears after the usual delay.
    pub fn destroy(&mut self) -> Option<Event> {
        let s = self.ship.take()?;
        self.ship_before = None;
        self.deaths += 1;
        self.respawn_tick = Some(self.tick + self.rules.respawn_ticks as u64);
        Some(Event::Crashed { x: s.p.x, y: s.p.y, vx: s.p.vx, vy: s.p.vy, body: u32::MAX })
    }

    /// Advance the world and the ship by one tick.
    pub fn step(&mut self, input: ShipInput) -> (Vec<Merge>, Vec<Event>) {
        let events = self.step_ship(input);
        (self.step_world(), events)
    }

    /// First half of a step, and the quick one: the ship moves on to tick + 1. It only
    /// needs the world as it is at the start of the tick.
    pub fn step_ship(&mut self, input: ShipInput) -> Vec<Event> {
        let mut events = Vec::new();
        if self.ship.is_none() && self.respawn_tick.is_some_and(|t| self.tick >= t) && self.spawn() {
            events.push(Event::Spawned);
        }
        self.ship_before = self.ship;
        if let Some(mut ship) = self.ship {
            {
                let b = self.bodies.read().unwrap();
                self.engine.local(&b, ship.p.x, ship.p.y, SHIP_THETA, self.near, &self.watch, &mut self.local);
            }
            self.local_tick = self.tick;
            let n = self.local.len();
            if n > LOCAL_MAX {
                self.near *= 0.8;
            } else if n < LOCAL_MIN {
                self.near = (self.near * 1.25).min(self.engine.extent());
            }
            self.alive.clear();
            self.alive.resize(n, true);
            self.no_radius.clear();
            self.no_radius.resize(n, 0.0);
            let l = &self.local;
            let radius = if self.god { &self.no_radius } else { &l.radius };
            let view = MassiveView { x: &l.x, y: &l.y, vx: &l.vx, vy: &l.vy, ax: &l.ax, ay: &l.ay, mass: &l.mass, radius, alive: &self.alive };
            if input.thrust > 0 {
                // The engine gives the same acceleration at any pace; keep the fraction of a
                // mm/s that a very short step cannot spend for the next one.
                let burn = THRUST * self.rules.dt * 1.0e3 + self.burn_carry;
                self.rules.burn_per_tick_mmps = burn.floor().min(1.0e15) as i64;
                self.burn_carry = burn - burn.floor();
            }
            let hit = step_ship(&mut ship, input, &view, &self.rules, &mut self.scratch);
            if self.god {
                ship.fuel = self.rules.fuel_max_mmps;
            }
            match hit.filter(|_| !self.god) {
                Some(slot) => {
                    self.ship = None;
                    self.deaths += 1;
                    self.respawn_tick = Some(self.tick + 1 + self.rules.respawn_ticks as u64);
                    let body = self.local.id[slot as usize];
                    events.push(Event::Crashed { x: ship.p.x, y: ship.p.y, vx: self.local.vx[slot as usize], vy: self.local.vy[slot as usize], body });
                }
                None => self.ship = Some(ship),
            }
        }
        events
    }

    /// Second half: the bodies move on to tick + 1.
    pub fn step_world(&mut self) -> Vec<Merge> {
        {
            let mut b = self.bodies.write().unwrap();
            self.engine.advance(&mut b, self.rules.dt);
            b.dt = self.rules.dt;
            b.tick = self.tick + 1;
        }
        {
            let b = self.bodies.read().unwrap();
            self.engine.forces(&b);
        }
        let merges = {
            let mut b = self.bodies.write().unwrap();
            self.engine.finish(&mut b, self.rules.dt)
        };
        for m in &merges {
            for w in self.watch.iter_mut().filter(|w| **w == m.absorbed) {
                *w = m.survivor;
            }
        }
        self.merges_total += merges.len() as u64;
        self.removed_total += self.engine.stats.removed as u64;
        self.tick += 1;
        self.time += self.rules.dt;
        merges
    }

    /// The ship's surroundings as a small world the exact engine can run ahead, plus the
    /// body id behind each of its slots (`u32::MAX` for lumps).
    pub fn local_snapshot(&self) -> (MassiveSnapshot, Vec<u32>) {
        let l = &self.local;
        let snapshot = MassiveSnapshot {
            tick: self.local_tick,
            x: l.x.clone(),
            y: l.y.clone(),
            vx: l.vx.clone(),
            vy: l.vy.clone(),
            mass: l.mass.clone(),
            // An indestructible ship flies through bodies; its predicted path should too.
            radius: if self.god { vec![0.0; l.len()] } else { l.radius.clone() },
            alive: vec![true; l.len()],
        };
        (snapshot, l.id.clone())
    }
}
