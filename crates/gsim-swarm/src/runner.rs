//! Runs a [`Sim`] on its own thread, as fast as the machine allows, and publishes what a
//! front end needs.
//!
//! There is no fixed step rate. Each step covers `pace x how long steps have been taking`,
//! so the world keeps the asked-for pace whether a step takes one millisecond or fifty;
//! unless that would be too long a step for the tightest bound orbit, in which case the
//! step is shortened and time runs slower than asked.

use crate::engine::{Bodies, Merge, Mode};
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
    /// Whether tight orbits are followed (by bodies taking shorter steps of their own).
    Tight(bool),
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
    /// Simulated seconds the step under way covers: time is not known to go on beyond that,
    /// however long the step takes.
    pub span: f64,
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
    /// Time is running slower than asked for.
    pub limited: bool,
    /// Bodies taking more than one step of their own in a step of the world, the most any
    /// takes, and what that costs.
    pub fine: u32,
    pub most: u32,
    pub own_ms: f32,
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
    pub tight: bool,
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
/// A step is stretched by at most this much to make up for what the ties cost.
const STRETCH: f64 = 3.0;
/// The ties' own steps may take this much of the time the rest of a step takes. Beyond that
/// time is slowed down for them.
const OWN_SHARE: f64 = 1.0;
/// What a step covers changes by at most this fraction from one step to the next, on the
/// way up: asked for a much higher pace, the world gets there over a few steps, and is seen
/// to be too much in time.
const GENTLY: f64 = 0.08;

/// Decides how long each step of the world is.
///
/// A step covers as much as a step takes, at the pace asked for: then the world keeps its
/// pace whether a step takes one millisecond or fifty. Part of a step goes on ties, and the
/// longer the step, the more there are and the more steps of their own each takes. Where
/// they are few, a somewhat longer step pays for them and the pace is held. Where they are
/// many it would only get longer and longer: so a step is never stretched beyond [`STRETCH`]
/// times what it takes without them, and it only grows while the ties take less than
/// [`OWN_SHARE`] of the rest, the more slowly the nearer they are to that, and shrinks when
/// they take more. Time then runs slower than asked for.
pub struct Pacer {
    /// Smoothed duration of a step (s), and of what of it is not ties.
    pub step_s: f64,
    base_s: f64,
    /// How much real time's worth a step covers (s).
    reach: f64,
    /// What the last step covered (s of the world).
    last: f64,
}

impl Default for Pacer {
    fn default() -> Self {
        // (The very first steps are short ones, whatever the pace.)
        Self { step_s: 0.002, base_s: 0.002, reach: 0.0002, last: 0.0 }
    }
}

impl Pacer {
    /// How long the next step is to be (s of the world). `longest` is what the engine allows
    /// (see `Engine::longest_step`).
    pub fn next(&self, pace: f64, longest: f64) -> f64 {
        let gently = if self.last > 0.0 { (1.0 + 3.0 * GENTLY) * self.last } else { f64::MAX };
        (pace * self.reach).min(longest).min(gently)
    }

    /// A step of `dt` took `took` seconds, `own` of them on ties.
    pub fn took(&mut self, dt: f64, took: f64, own: f64) {
        let rest = (took - own).max(0.0);
        self.step_s += 0.15 * (took - self.step_s);
        self.base_s += 0.15 * (rest - self.base_s);
        self.last = dt;
        // What would hold the pace, and how much room the ties leave for getting there.
        let wanted = self.step_s.min(STRETCH * self.base_s).min(LONGEST_STEP);
        let room = (1.0 - own / (OWN_SHARE * rest).max(1.0e-9)).clamp(-1.0, 1.0);
        let grown = self.reach * (1.0 + GENTLY * room);
        self.reach = if grown > wanted { self.reach + 0.15 * (wanted - self.reach) } else { grown };
    }

}


impl Runner {
    /// `threads` is how many the simulation may use (see [`Command::Threads`]); `pace` the
    /// simulated seconds per real second to start with.
    pub fn start(sim: Sim, pace: f64, threads: usize) -> Self {
        let bodies = sim.bodies.clone();
        let reading = sim.reader_waiting.clone();
        let published = Arc::new(Mutex::new(Published {
            clock: Clock { time: 0.0, at: Instant::now(), rate: 0.0, span: 0.0 },
            time: 0.0,
            pace,
            theta: sim.engine.theta,
            tight: sim.engine.own_steps,
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
    /// Sums of sort, force and finish milliseconds, of interactions per body, and of the
    /// milliseconds spent on bodies taking steps of their own.
    parts: [f64; 5],
    merges: usize,
}

impl Window {
    fn new() -> Self {
        Self { since: Instant::now(), steps: 0, advanced: 0.0, busy: 0.0, parts: [0.0; 5], merges: 0 }
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
    fine: u32,
    most: u32,
    own_ms: f32,
    merges_per_s: f32,
}

fn run(mut sim: Sim, inbox: Receiver<Command>, out: Arc<Mutex<Published>>, pace: f64, threads: usize) {
    // The simulation has its own threads, so that whoever draws it can decide how many cores
    // are left over for that.
    let mut workers = pool(threads);
    let (mut pace, mut paused) = (pace, false);
    let mut pacer = Pacer::default();
    // What is shown is averaged over windows of half a second, so that it can be read.
    let mut window = Window::new();
    let mut shown = Shown::default();
    let mut last_error = None;
    let (mut last_heaviest, mut last_error_at) = (Instant::now() - Duration::from_secs(1), Instant::now());
    let mut notice = None;
    // Before the first step, find out how long a step the world as it starts can take.
    sim.engine.dt_hint = pacer.next(pace, f64::MAX);
    workers.install(|| sim.refresh());
    loop {
        let mut edited = false;
        loop {
            match inbox.try_recv() {
                Ok(Command::Pace(p)) if p > 0.0 && p.is_finite() => (pace, paused) = (p, false),
                Ok(Command::Pace(_)) => paused = true,
                Ok(Command::Theta(t)) => sim.engine.theta = t.clamp(0.2, 1.5),
                Ok(Command::DropLeavers(on)) => sim.engine.drop_leavers = on,
                Ok(Command::Tight(on)) => sim.engine.own_steps = on,
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
                        Command::Pace(_) | Command::Theta(_) | Command::Tight(_) | Command::DropLeavers(_) | Command::Threads(_) | Command::Watch(_) => {}
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
            p.clock = Clock { time: sim.time, at: now, rate: 0.0, span: 0.0 };
            (p.time, p.pace, p.theta, p.tight, p.watch) = (sim.time, 0.0, sim.engine.theta, sim.engine.own_steps, sim.watch.clone());
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

        let dt = pacer.next(pace, sim.engine.longest_step());
        let step_s = pacer.step_s;
        sim.engine.dt_hint = dt;
        sim.engine.fade_step = (step_s / FADE_SECONDS) as f32;
        let started = Instant::now();
        sim.let_readers_in();
        out.lock().unwrap().clock = Clock { time: sim.time, at: started, rate: dt / step_s, span: dt };
        // A few bodies are stepped right here: handing that to other threads would cost more
        // than doing it.
        let merges = if sim.small() { sim.step(dt) } else { workers.install(|| sim.step(dt)) };
        if sim.bodies.read().unwrap().is_empty() {
            // Nothing to do, and no reason to do it a million times a second.
            std::thread::sleep(Duration::from_millis(1));
        }
        let took = started.elapsed().as_secs_f64();
        pacer.took(dt, took, sim.engine.stats.own_ms as f64 * 1.0e-3);
        let s = sim.engine.stats;
        window.steps += 1;
        window.advanced += dt;
        window.busy += took;
        for (sum, part) in window.parts.iter_mut().zip([s.sort_ms, s.force_ms, s.finish_ms, s.interactions, s.own_ms]) {
            *sum += part as f64;
        }
        window.merges += merges.len();
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
                // (Said to be so from a tenth short, and no longer once nearly made up: a pace
                // just at the edge would have it change every half second.)
                limited: window.advanced / span < pace * if shown.limited { 0.97 } else { 0.9 },
                fine: s.fine,
                most: s.most,
                own_ms: (window.parts[4] / steps) as f32,
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
        (p.time, p.pace, p.theta, p.tight, p.watch) = (sim.time, pace, sim.engine.theta, sim.engine.own_steps, sim.watch.clone());
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
            fine: shown.fine,
            most: shown.most,
            own_ms: shown.own_ms,
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
