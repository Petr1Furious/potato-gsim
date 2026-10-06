//! Runs a [`Sim`] on its own thread at a chosen pace and publishes what a front end needs.

use crate::engine::{Bodies, Merge};
use crate::sim::{Event, Sim};
use gsim_core::{GameRules, MassiveSnapshot, Particle, ShipInput, ShipState};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub enum Command {
    Input(ShipInput),
    /// Simulated seconds per real second; 0 pauses. Steps stay as frequent and cover more
    /// or less time each.
    Pace(f64),
    Theta(f32),
    God(bool),
    Place(Particle),
    /// Park the ship on an orbit around the body with this id.
    Orbit(u32),
    Refuel,
    Destroy,
    Watch(Vec<u32>),
}

/// Maps wall-clock time to simulation ticks between two steps.
#[derive(Clone, Copy)]
pub struct Clock {
    pub tick: f64,
    pub at: Instant,
    /// Ticks per second the simulation is expected to manage.
    pub rate: f64,
}

#[derive(Clone, Default)]
pub struct Stats {
    pub bodies: usize,
    pub step_ms: f32,
    pub sort_ms: f32,
    pub force_ms: f32,
    pub finish_ms: f32,
    pub interactions: f32,
    /// Ticks per second actually achieved, and asked for.
    pub rate: f32,
    pub wanted_rate: f32,
    pub merges_total: u64,
    pub removed_total: u64,
    pub merges_per_s: f32,
    pub deaths: u32,
    /// Relative force error against exact sums, sampled now and then.
    pub error: Option<f32>,
    pub level: &'static str,
    pub local: usize,
}

#[derive(Clone, Copy)]
pub struct Heavy {
    pub id: u32,
    pub mass: f32,
    pub radius: f32,
}

/// The ship's surroundings at `snapshot.tick`, for predicting its path.
pub struct LocalWorld {
    pub snapshot: MassiveSnapshot,
    /// Body id of each slot (`u32::MAX` for lumps).
    pub ids: Vec<u32>,
}

pub struct Published {
    pub clock: Clock,
    /// Ticks completed.
    pub tick: u64,
    /// The ship at `ship_tick` and one tick earlier (the same state twice right after it
    /// appeared, or while paused).
    pub ship: Option<ShipState>,
    pub ship_before: Option<ShipState>,
    pub ship_tick: u64,
    pub respawn_tick: Option<u64>,
    pub god: bool,
    /// Simulated seconds per real second being asked for (0 while paused).
    pub pace: f64,
    /// Seconds covered by the step between `ship_before` and `ship`.
    pub ship_dt: f64,
    /// Simulated seconds since the start.
    pub time: f64,
    pub theta: f32,
    pub stats: Stats,
    pub heaviest: Vec<Heavy>,
    /// Since the front end last drained them.
    pub events: Vec<Event>,
    pub merges: Vec<(Merge, u64)>,
    pub local: Option<Arc<LocalWorld>>,
    pub watch: Vec<u32>,
}

impl Published {
    /// The moment to draw, in ticks. Never ahead of what has been computed.
    pub fn present(&self, now: Instant, computed: u64) -> f64 {
        let t = self.clock.tick + now.saturating_duration_since(self.clock.at).as_secs_f64() * self.clock.rate;
        t.min(computed as f64)
    }
}

pub struct Runner {
    pub bodies: Arc<RwLock<Bodies>>,
    pub published: Arc<Mutex<Published>>,
    pub rules: GameRules,
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

const MERGE_BACKLOG: usize = 6000;
const HEAVIEST: usize = 12;

impl Runner {
    pub fn start(sim: Sim) -> Self {
        let bodies = sim.bodies.clone();
        let rules = sim.rules.clone();
        let published = Arc::new(Mutex::new(Published {
            clock: Clock { tick: 0.0, at: Instant::now(), rate: 0.0 },
            tick: 0,
            ship: None,
            ship_before: None,
            ship_tick: 0,
            respawn_tick: None,
            god: false,
            pace: sim.rules.time_scale(),
            ship_dt: sim.rules.dt,
            time: 0.0,
            theta: sim.engine.theta,
            stats: Stats { level: sim.engine.level().name(), ..Default::default() },
            heaviest: Vec::new(),
            events: Vec::new(),
            merges: Vec::new(),
            local: None,
            watch: Vec::new(),
        }));
        let (commands, inbox) = channel();
        let out = published.clone();
        let thread = std::thread::Builder::new().name("gsim-swarm".into()).spawn(move || run(sim, inbox, out)).expect("spawn simulation thread");
        Self { bodies, published, rules, commands, thread: Some(thread) }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        // Closing the channel is the stop signal.
        let (dead, _) = channel();
        self.commands = dead;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(mut sim: Sim, inbox: Receiver<Command>, out: Arc<Mutex<Published>>) {
    let hz = sim.rules.tick_hz as f64;
    let mut input = ShipInput::default();
    let mut paused = false;
    let mut due = Instant::now();
    // Smoothed duration of a step, and merges per second.
    let mut step_s = 1.0 / hz;
    let mut merge_rate = 0.0f32;
    let mut last_error = None;
    let mut last_local = 0u64;
    loop {
        let mut events = Vec::new();
        loop {
            match inbox.try_recv() {
                Ok(Command::Input(i)) => input = i,
                Ok(Command::Pace(pace)) => {
                    paused = pace <= 0.0 || pace.is_nan();
                    sim.set_pace(pace);
                }
                Ok(Command::Theta(t)) => sim.engine.theta = t.clamp(0.2, 1.5),
                Ok(Command::God(on)) => sim.god = on,
                Ok(Command::Place(p)) => sim.place(p),
                Ok(Command::Orbit(id)) => {
                    sim.orbit(id);
                }
                Ok(Command::Refuel) => sim.refuel(),
                Ok(Command::Destroy) => events.extend(sim.destroy()),
                Ok(Command::Watch(ids)) => sim.watch = ids,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
        let now = Instant::now();
        if paused || now < due {
            {
                // Commands take effect on what is shown even while time stands still.
                let mut p = out.lock().unwrap();
                p.pace = if paused { 0.0 } else { sim.rules.time_scale() };
                p.god = sim.god;
                p.theta = sim.engine.theta;
                p.events.append(&mut events);
                if paused {
                    p.clock = Clock { tick: sim.tick as f64, at: now, rate: 0.0 };
                    p.ship = sim.ship;
                    p.ship_before = sim.ship;
                    p.ship_tick = sim.tick;
                    p.respawn_tick = sim.respawn_tick;
                    p.watch = sim.watch.clone();
                    due = now;
                }
            }
            let wait = if paused { Duration::from_millis(4) } else { (due - now).min(Duration::from_millis(2)) };
            std::thread::sleep(wait);
            continue;
        }
        let wanted = hz;
        // Promise only the pace we have been keeping, so motion stays even when overloaded.
        let rate = wanted.min(1.0 / step_s);
        let started = Instant::now();
        let before = sim.ship;
        // The ship is quick: publish where it will be before the long part starts, so it can
        // be drawn moving while the bodies are being computed.
        events.append(&mut sim.step_ship(input));
        {
            let mut p = out.lock().unwrap();
            p.clock = Clock { tick: sim.tick as f64, at: started, rate };
            p.ship = sim.ship;
            p.ship_before = before.filter(|_| sim.ship.is_some()).or(sim.ship);
            p.ship_tick = sim.tick + 1;
            p.ship_dt = sim.rules.dt;
            p.respawn_tick = sim.respawn_tick;
        }
        let merges = sim.step_world();
        let took = started.elapsed().as_secs_f64();
        step_s += 0.1 * (took - step_s);
        merge_rate += 0.05 * (merges.len() as f32 * rate as f32 - merge_rate);
        if sim.tick % 900 == 450 {
            let b = sim.bodies.read().unwrap();
            last_error = Some(sim.engine.force_error(&b, 24) as f32);
        }
        let heaviest = (sim.tick % 30 == 1).then(|| {
            let b = sim.bodies.read().unwrap();
            b.heaviest(HEAVIEST).into_iter().map(|i| Heavy { id: b.id[i], mass: b.m[i], radius: b.r[i] }).collect::<Vec<_>>()
        });
        // A fresh picture of the ship's surroundings a few times a second.
        let local = (sim.ship.is_some() && sim.local_tick + 1 == sim.tick && sim.tick >= last_local + 12).then(|| {
            last_local = sim.tick;
            let (snapshot, ids) = sim.local_snapshot();
            Arc::new(LocalWorld { snapshot, ids })
        });
        {
            let mut p = out.lock().unwrap();
            p.tick = sim.tick;
            p.ship = sim.ship;
            p.ship_before = before.filter(|_| sim.ship.is_some()).or(sim.ship);
            p.ship_tick = sim.tick;
            p.respawn_tick = sim.respawn_tick;
            p.god = sim.god;
            p.pace = sim.rules.time_scale();
            p.time = sim.time;
            p.theta = sim.engine.theta;
            p.watch = sim.watch.clone();
            let s = sim.engine.stats;
            p.stats = Stats {
                bodies: sim.bodies.read().unwrap().len(),
                step_ms: (step_s * 1e3) as f32,
                sort_ms: s.sort_ms,
                force_ms: s.force_ms,
                finish_ms: s.finish_ms,
                interactions: s.interactions,
                rate: (1.0 / step_s.max(1.0 / wanted)) as f32,
                wanted_rate: wanted as f32,
                merges_total: sim.merges_total,
                removed_total: sim.removed_total,
                merges_per_s: merge_rate,
                deaths: sim.deaths,
                error: last_error,
                level: sim.engine.level().name(),
                local: sim.local.len(),
            };
            if let Some(h) = heaviest {
                p.heaviest = h;
            }
            if local.is_some() {
                p.local = local;
            }
            p.events.append(&mut events);
            let tick = sim.tick;
            p.merges.extend(merges.into_iter().map(|m| (m, tick)));
            if p.merges.len() > MERGE_BACKLOG {
                let extra = p.merges.len() - MERGE_BACKLOG;
                p.merges.drain(..extra);
            }
        }
        // Late steps are forgiven rather than caught up on.
        due = (due + Duration::from_secs_f64(1.0 / wanted)).max(Instant::now() - Duration::from_millis(1));
    }
}
