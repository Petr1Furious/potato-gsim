//! Runs a [`Sim`] on its own thread, as fast as the machine allows, and publishes what a
//! front end needs.
//!
//! There is no fixed step rate. Each step covers `pace x how long steps have been taking`,
//! so the world keeps the asked-for pace whether a step takes one millisecond or fifty;
//! unless that would be too long a step for the tightest bound orbit, in which case the
//! step is shortened and time runs slower than asked.

use crate::engine::{Bodies, Merge, Mode, ORBIT_FRACTION};
use crate::sim::Sim;

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub enum Command {
    /// Simulated seconds per real second; 0 pauses.
    Pace(f64),
    Theta(f32),
    /// Whether bodies leaving the world for good fade out and are dropped.
    DropLeavers(bool),
    /// How many threads the simulation may use from now on.
    Threads(usize),
    Watch(Vec<u32>),
    Add(Box<Bodies>),
    Erase { x: f64, y: f64, r: f64 },
    Remove(u32),
    Shatter { id: u32, pieces: usize, violence: f64 },
    Edit { id: u32, mass: f64, radius: f64, vx: f64, vy: f64 },
    Save(PathBuf),
    Load(PathBuf),

}

/// Maps wall-clock time to simulated time between two steps.
#[derive(Clone, Copy)]
pub struct Clock {
    pub time: f64,
    pub at: Instant,
    /// Simulated seconds per real second being managed right now.
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
    pub steps_per_s: f32,
    /// Simulated seconds the last step covered.
    pub dt: f64,
    /// Simulated seconds per real second achieved.
    pub achieved: f64,
    /// The step is being held short for the sake of a tight orbit.
    pub limited: bool,
    pub merges_total: u64,
    pub removed_total: u64,
    pub merges_per_s: f32,
    /// Relative force error against exact sums, sampled now and then (tree mode only).
    pub error: Option<f32>,
    pub level: &'static str,
    pub mode: &'static str,

}

#[derive(Clone, Copy)]
pub struct Heavy {
    pub id: u32,
    pub mass: f32,
    pub radius: f32,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
}

pub struct Published {
    pub clock: Clock,
    /// Simulated seconds since the start.
    pub time: f64,
    /// Simulated seconds per real second asked for (0 while paused).
    pub pace: f64,
    pub theta: f32,
    pub stats: Stats,
    pub heaviest: Vec<Heavy>,
    /// Merges since the front end last drained them, with the simulated time of each.
    pub merges: Vec<(Merge, f64)>,
    pub watch: Vec<u32>,
    /// Something to tell the player (a save that failed, a world loaded), once.
    pub notice: Option<String>,
}

pub struct Runner {
    pub bodies: Arc<RwLock<Bodies>>,
    pub published: Arc<Mutex<Published>>,
    /// Raise this before reading `bodies` or `published` and lower it afterwards: the
    /// simulation, which may be taking those locks hundreds of thousands of times a second,
    /// then stands aside.
    pub reading: Arc<AtomicBool>,
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

const MERGE_BACKLOG: usize = 6000;
const HEAVIEST: usize = 12;
/// However slow steps get, one never covers more than this much real time's worth.
/// Real seconds a body found leaving the world takes to fade away.
const FADE_SECONDS: f64 = 5.0;
const LONGEST_STEP: f64 = 0.1;


impl Runner {
    /// `threads` is how many the simulation may use (see [`Command::Threads`]); `pace` the
    /// simulated seconds per real second to start with.
    pub fn start(sim: Sim, pace: f64, threads: usize) -> Self {
        let bodies = sim.bodies.clone();
        let reading = sim.reader_waiting.clone();
        let published = Arc::new(Mutex::new(Published {
            clock: Clock { time: 0.0, at: Instant::now(), rate: 0.0 },
            time: 0.0,
            pace,
            theta: sim.engine.theta,
            stats: Stats { level: sim.engine.level().name(), ..Default::default() },
            heaviest: Vec::new(),
            merges: Vec::new(),
            watch: Vec::new(),
            notice: None,
        }));
        let (commands, inbox) = channel();
        let out = published.clone();
        let thread = std::thread::Builder::new().name("gsim-swarm".into()).spawn(move || run(sim, inbox, out, pace, threads)).expect("spawn simulation thread");
        Self { bodies, published, reading, commands, thread: Some(thread) }
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

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(threads.max(1)).thread_name(|i| format!("gsim-swarm-{i}")).build().expect("simulation threads")
}

fn save(bodies: &Bodies, path: &PathBuf) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    bodies.write(&mut out)
}

/// Seconds over which the published figures are averaged.
const WINDOW: f64 = 0.5;

/// What has happened since the published figures were last worked out.
struct Window {
    since: Instant,
    steps: u32,
    /// Simulated seconds covered.
    advanced: f64,
    /// Real seconds spent stepping.
    busy: f64,
    /// Sums of sort, force and finish milliseconds, and of interactions per body.
    parts: [f64; 4],
    merges: usize,
    /// Steps held short for a tight orbit.
    held: u32,
}

impl Window {
    fn new() -> Self {
        Self { since: Instant::now(), steps: 0, advanced: 0.0, busy: 0.0, parts: [0.0; 4], merges: 0, held: 0 }
    }
}

/// The figures of the last whole window.
#[derive(Clone, Copy, Default)]
struct Shown {
    step_ms: f32,
    sort_ms: f32,
    force_ms: f32,
    finish_ms: f32,
    interactions: f32,
    steps_per_s: f32,
    dt: f64,
    achieved: f64,
    limited: bool,
    merges_per_s: f32,
}

fn run(mut sim: Sim, inbox: Receiver<Command>, out: Arc<Mutex<Published>>, pace: f64, threads: usize) {
    // The simulation has its own threads, so that whoever draws it can decide how many cores
    // are left over for that.
    let mut workers = pool(threads);
    let (mut pace, mut paused) = (pace, false);
    // Smoothed duration of a step, and merges per second.
    let mut step_s = 0.002f64;
    // What is shown is averaged over windows of half a second, so that it can be read.
    let mut window = Window::new();
    let mut shown = Shown::default();
    let mut last_error = None;
    let (mut last_heaviest, mut last_error_at) = (Instant::now() - Duration::from_secs(1), Instant::now());
    let mut notice = None;
    // Before the first step, find out how long a step the world as it starts can take.
    sim.engine.dt_hint = pace * step_s;
    workers.install(|| sim.refresh());
    loop {
        let mut edited = false;
        loop {
            match inbox.try_recv() {
                Ok(Command::Pace(p)) if p > 0.0 && p.is_finite() => (pace, paused) = (p, false),
                Ok(Command::Pace(_)) => paused = true,
                Ok(Command::Theta(t)) => sim.engine.theta = t.clamp(0.2, 1.5),
                Ok(Command::DropLeavers(on)) => sim.engine.drop_leavers = on,
                Ok(Command::Threads(n)) => {
                    if n.max(1) != workers.current_num_threads() {
                        workers = pool(n);
                    }
                }
                Ok(Command::Watch(ids)) => sim.watch = ids,
                Ok(edit) => {
                    edited = true;
                    workers.install(|| match edit {
                        Command::Add(new) => sim.add(&new),
                        Command::Erase { x, y, r } => {
                            sim.erase(x, y, r);
                        }
                        Command::Remove(id) => {
                            sim.remove(id);
                        }
                        Command::Shatter { id, pieces, violence } => {
                            sim.shatter(id, pieces, violence);
                        }
                        Command::Edit { id, mass, radius, vx, vy } => {
                            sim.edit(id, mass, radius, vx, vy);
                        }
                        Command::Save(path) => {
                            let result = save(&sim.bodies.read().unwrap(), &path);
                            notice = Some(match result {
                                Ok(()) => format!("Saved {}", path.file_stem().unwrap_or_default().to_string_lossy()),
                                Err(e) => format!("Could not save: {e}"),
                            });
                        }
                        Command::Load(path) => {
                            let loaded = std::fs::File::open(&path).and_then(|f| Bodies::read(&mut std::io::BufReader::new(f)));
                            notice = Some(match loaded {
                                Ok(bodies) => {
                                    sim.replace(bodies);

                                    format!("Loaded {}", path.file_stem().unwrap_or_default().to_string_lossy())
                                }
                                Err(e) => format!("Could not load: {e}"),
                            });
                        }
                        Command::Pace(_) | Command::Theta(_) | Command::DropLeavers(_) | Command::Threads(_) | Command::Watch(_) => {}
                    });
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
        let now = Instant::now();
        // The heavyweights, a few times a second: the front end lists them and aims by them.
        let heaviest = (edited || now.duration_since(last_heaviest) > Duration::from_millis(250)).then(|| {
            last_heaviest = now;
            let b = sim.bodies.read().unwrap();
            workers.install(|| b.heaviest(HEAVIEST)).into_iter().map(|i| Heavy { id: b.id[i], mass: b.m[i], radius: b.r[i], x: b.x[i], y: b.y[i], vx: b.vx[i], vy: b.vy[i] }).collect::<Vec<_>>()
        });
        if paused {
            window = Window::new();
            let mut p = out.lock().unwrap();
            p.clock = Clock { time: sim.time, at: now, rate: 0.0 };
            (p.time, p.pace, p.theta, p.watch) = (sim.time, 0.0, sim.engine.theta, sim.watch.clone());
            p.stats.bodies = sim.bodies.read().unwrap().len();

            if let Some(h) = heaviest {
                p.heaviest = h;
            }
            if notice.is_some() {
                p.notice = notice.take();
            }
            drop(p);
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }

        // How long a step this one should be to hold the pace, and how long it may be.
        let wanted = pace * step_s.min(LONGEST_STEP);
        let limit = if sim.engine.omega2 > 0.0 { ORBIT_FRACTION / sim.engine.omega2.sqrt() } else { f64::MAX };
        let dt = wanted.min(limit);
        sim.engine.dt_hint = wanted;
        sim.engine.fade_step = (step_s / FADE_SECONDS) as f32;
        let started = Instant::now();
        sim.let_readers_in();
        out.lock().unwrap().clock = Clock { time: sim.time, at: started, rate: dt / step_s };
        // A few bodies are stepped right here: handing that to other threads would cost more
        // than doing it.
        let merges = if sim.small() { sim.step(dt) } else { workers.install(|| sim.step(dt)) };
        if sim.bodies.read().unwrap().is_empty() {
            // Nothing to do, and no reason to do it a million times a second.
            std::thread::sleep(Duration::from_millis(1));
        }
        let took = started.elapsed().as_secs_f64();
        step_s += 0.15 * (took - step_s);
        let s = sim.engine.stats;
        window.steps += 1;
        window.advanced += dt;
        window.busy += took;
        window.parts = [window.parts[0] + s.sort_ms as f64, window.parts[1] + s.force_ms as f64, window.parts[2] + s.finish_ms as f64, window.parts[3] + s.interactions as f64];
        window.merges += merges.len();
        window.held += (limit < wanted) as u32;
        let span = window.since.elapsed().as_secs_f64();
        if span >= WINDOW {
            let steps = window.steps as f64;
            shown = Shown {
                step_ms: (window.busy / steps * 1e3) as f32,
                sort_ms: (window.parts[0] / steps) as f32,
                force_ms: (window.parts[1] / steps) as f32,
                finish_ms: (window.parts[2] / steps) as f32,
                interactions: (window.parts[3] / steps) as f32,
                steps_per_s: (steps / span) as f32,
                dt: window.advanced / steps,
                achieved: window.advanced / span,
                limited: window.held * 4 > window.steps,
                merges_per_s: (window.merges as f64 / span) as f32,
            };
            window = Window::new();
        }
        if sim.engine.mode == Mode::Tree && now.duration_since(last_error_at) > Duration::from_secs(8) {
            last_error_at = now;
            let b = sim.bodies.read().unwrap();
            last_error = Some(workers.install(|| sim.engine.force_error(&b, 24)) as f32);
        } else if sim.engine.mode != Mode::Tree {
            last_error = Some(0.0);
        }
        sim.let_readers_in();
        let mut p = out.lock().unwrap();
        (p.time, p.pace, p.theta, p.watch) = (sim.time, pace, sim.engine.theta, sim.watch.clone());
        p.stats = Stats {
            bodies: sim.bodies.read().unwrap().len(),
            step_ms: shown.step_ms,
            sort_ms: shown.sort_ms,
            force_ms: shown.force_ms,
            finish_ms: shown.finish_ms,
            interactions: shown.interactions,
            steps_per_s: shown.steps_per_s,
            dt: shown.dt,
            achieved: shown.achieved,
            limited: shown.limited,
            merges_total: sim.merges_total,
            removed_total: sim.removed_total,
            merges_per_s: shown.merges_per_s,
            error: last_error,
            level: sim.engine.level().name(),
            mode: sim.engine.mode.name(),

        };
        if let Some(h) = heaviest {
            p.heaviest = h;
        }
        if notice.is_some() {
            p.notice = notice.take();
        }
        let time = sim.time;
        p.merges.extend(merges.into_iter().map(|m| (m, time)));
        if p.merges.len() > MERGE_BACKLOG {
            let extra = p.merges.len() - MERGE_BACKLOG;
            p.merges.drain(..extra);
        }
    }
}
