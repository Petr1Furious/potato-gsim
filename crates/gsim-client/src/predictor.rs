//! Background trajectory prediction. Exact prediction against a thousand bodies for twenty
//! seconds ahead costs tens of milliseconds, so it runs off the render thread; the newest
//! finished result is drawn, re-anchored to the reference body's current position. (A page in
//! a browser has no other thread: there a prediction is worked out a little each frame, for
//! a part of the frame's time, and shown when it is whole.)
//!
//! A ship that is coasting stays on the path predicted for it: then only the few ticks beyond
//! the end of that path are worked out, not the whole of it again.

use gsim_client_core::eph::EphReader;
use gsim_core::particle::step_particle;
use gsim_core::predict::Path;
use gsim_core::ship::step_ship;
use gsim_core::{GameRules, InputTimeline, Particle, Scratch, ShipInput, ShipState, Tick};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

pub struct Job {
    pub reader: EphReader,
    pub rules: GameRules,
    /// Newest simulated state of our ship and its tick.
    pub ship: ShipState,
    pub start: Tick,
    pub timeline: InputTimeline,
    pub ticks: u32,
    pub show_ship: bool,
    /// Also predict with the scheduled thrust kept on, until it has spent `burn_mmps`.
    pub held: bool,
    pub burn_mmps: i64,
    /// Muzzle state of a shell fired now, if a preview is wanted.
    pub shell: Option<Particle>,
    /// Draw everything relative to this body.
    pub ref_slot: Option<u32>,
}

/// Paths as offsets from the reference body at each point's own tick (absolute if none).
#[derive(Default)]
pub struct Paths {
    pub ref_slot: Option<u32>,
    pub coast: Vec<(f64, f64)>,
    pub coast_impact: bool,
    pub held: Vec<(f64, f64)>,
    pub shell: Vec<(f64, f64)>,
    /// Index into `coast` and distance of the closest approach to the reference body.
    pub closest: Option<(usize, f64)>,
    /// The same for the first point from which the path turns back towards the reference body.
    pub farthest: Option<(usize, f64)>,
    /// After `farthest` the path really does come back: at least half way to where it was
    /// nearest before. A path that merely wavers on its way out does not.
    pub returns: bool,
    pub compute_ms: f64,
}

#[cfg(not(target_arch = "wasm32"))]
pub struct Predictor {
    tx: Sender<Job>,
    rx: Receiver<Paths>,
    busy: bool,
    pub latest: Paths,
}

#[cfg(target_arch = "wasm32")]
pub struct Predictor {
    /// The prediction being worked out, if it is not finished.
    doing: Option<Doing>,
    kept: Option<Coast>,
    pub latest: Paths,
}

fn relative(job: &Job, path: &Path) -> Vec<(f64, f64)> {
    let Some(mut slot) = job.ref_slot.map(|s| s as usize) else { return path.points.clone() };
    let mut last = (0.0, 0.0);
    // False once the reference has been annihilated or removed: it then stays where it was.
    let mut exists = true;
    path.points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let tick = path.start_tick + i as Tick;
            let row = job.reader.get(tick);
            let before = tick.checked_sub(1).and_then(|t| job.reader.get(t));
            // If the reference merged into another body during the previous tick, measure
            // against that body from here on (a dead slot would otherwise read as the origin).
            if let Some(event) = before.as_ref().and_then(|r| r.merges.iter().find(|e| e.absorbed.contains(&(slot as u32)))) {
                match event.survivor {
                    Some(s) => slot = s as usize,
                    None => exists = false,
                }
            }
            if !exists {
            } else if let Some(row) = &row {
                last = (row.x[slot], row.y[slot]);
            } else if let Some(row) = &before {
                // One past the newest row: where the body ends up after that tick.
                let dt = job.rules.dt;
                last = (
                    row.x[slot] + dt * (row.vx[slot] + 0.5 * row.ax[slot] * dt),
                    row.y[slot] + dt * (row.vy[slot] + 0.5 * row.ay[slot] * dt),
                );
            }
            (p.0 - last.0, p.1 - last.1)
        })
        .collect()
}

/// The coasting path worked out last time, tick by tick from `start`.
struct Coast {
    reader: EphReader,
    start: Tick,
    states: Vec<ShipState>,
    impact: Option<u32>,
}

/// Is it time to stop for now? (Never, without a time to stop at.)
fn late(until: f64) -> bool {
    until.is_finite() && crate::fmt::now() >= until
}

/// Where the ship goes with its engine off. Picks up the path kept from last time if the
/// ship is on it (to the bit: the prediction is what then happens). `None` if `until` came
/// first: the next call goes on from there.
fn coast(job: &Job, kept: &mut Option<Coast>, until: f64) -> Option<Path> {
    let on = kept.as_ref().filter(|k| k.reader.same(&job.reader)).and_then(|k| {
        let at = job.start.checked_sub(k.start)? as usize;
        (k.states.get(at) == Some(&job.ship)).then_some(at)
    });
    let k = match (kept.take(), on) {
        (Some(mut k), Some(at)) => {
            k.states.drain(..at);
            k.start = job.start;
            kept.insert(k)
        }
        _ => kept.insert(Coast { reader: job.reader.clone(), start: job.start, states: vec![job.ship], impact: None }),
    };
    let mut scratch = Scratch::default();
    while k.impact.is_none() && k.states.len() <= job.ticks as usize {
        if late(until) {
            return None;
        }
        let Some(row) = job.reader.get(k.start + k.states.len() as Tick - 1) else { break };
        let mut s = *k.states.last().unwrap();
        k.impact = step_ship(&mut s, ShipInput::default(), &row.view(), &job.rules, &mut scratch);
        k.states.push(s);
    }
    Some(Path { start_tick: k.start, points: k.states.iter().map(|s| (s.p.x, s.p.y)).collect(), impact: k.impact })
}

/// A path being worked out tick by tick, which can be left and gone on with.
struct Walk<S> {
    state: S,
    path: Path,
    done: bool,
}

impl<S> Walk<S> {
    fn new(state: S, start: Tick, at: (f64, f64)) -> Self {
        Self { state, path: Path { start_tick: start, points: vec![at], impact: None }, done: false }
    }

    /// Go on for up to `ticks` in all, or until the ephemeris runs out, something is hit
    /// or it is `until`. True once there is no more to do. `step` advances the state by a
    /// tick and says where it then is and what it ran into.
    fn go(&mut self, job: &Job, ticks: u32, until: f64, mut step: impl FnMut(&mut S, Tick, &gsim_core::EphRow) -> ((f64, f64), Option<u32>)) -> bool {
        while !self.done {
            if late(until) {
                return false;
            }
            let tick = self.path.start_tick + self.path.points.len() as Tick - 1;
            let row = job.reader.get(tick).filter(|_| self.path.points.len() <= ticks as usize);
            let Some(row) = row else { break };
            let (at, hit) = step(&mut self.state, tick, &row);
            self.path.points.push(at);
            (self.path.impact, self.done) = (hit, hit.is_some());
        }
        self.done = true;
        true
    }
}

/// A prediction under way.
struct Doing {
    job: Job,
    coast: Option<Path>,
    held: Walk<ShipState>,
    shell: Option<Walk<Particle>>,
    /// Milliseconds worked on it so far.
    spent: f64,
}

impl Doing {
    fn new(job: Job) -> Self {
        let held = Walk::new(job.ship, job.start, (job.ship.p.x, job.ship.p.y));
        let shell = job.shell.map(|p| Walk::new(p, job.start, (p.x, p.y)));
        Self { job, coast: None, held, shell, spent: 0.0 }
    }

    /// Work on it until `until` (seconds on [`crate::fmt::now`]): the paths, once whole.
    fn go(&mut self, kept: &mut Option<Coast>, until: f64) -> Option<Paths> {
        let t0 = crate::fmt::now();
        let whole = self.paths(kept, until);
        self.spent += (crate::fmt::now() - t0) * 1e3;
        whole.map(|paths| Paths { compute_ms: self.spent, ..paths })
    }

    fn paths(&mut self, kept: &mut Option<Coast>, until: f64) -> Option<Paths> {
        let job = &self.job;
        let mut scratch = Scratch::default();
        if job.show_ship {
            if self.coast.is_none() {
                self.coast = Some(coast(job, kept, until)?);
            }
            // The burn goes on until it has cost so much, and then the ship coasts.
            let burning_until = job.ship.fuel - job.burn_mmps;
            let whole = !job.held
                || self.held.go(job, job.ticks, until, |s, tick, row| {
                    let input = if s.fuel > burning_until { job.timeline.at(tick) } else { ShipInput::default() };
                    let hit = step_ship(s, input, &row.view(), &job.rules, &mut scratch);
                    ((s.p.x, s.p.y), hit)
                });
            if !whole {
                return None;
            }
        }
        if let Some(shell) = &mut self.shell {
            let life = job.rules.shell_lifetime_ticks.min(job.ticks);
            let whole = shell.go(job, life, until, |p, _, row| {
                let hit = step_particle(p, (0.0, 0.0), job.rules.shell_radius, &row.view(), &job.rules, &mut scratch);
                ((p.x, p.y), hit)
            });
            if !whole {
                return None;
            }
        }
        let mut out = Paths { ref_slot: job.ref_slot, ..Default::default() };
        if let Some(coast) = &self.coast {
            out.coast_impact = coast.impact.is_some();
            out.coast = relative(job, coast);
            if job.ref_slot.is_some() {
                // The next close approach, not the lowest point anywhere in the window: on a
                // repeating orbit later passes are about as close and would win by a hair.
                let d: Vec<f64> = out.coast.iter().map(|q| (q.0 * q.0 + q.1 * q.1).sqrt()).collect();
                out.closest = (1..d.len().saturating_sub(1)).find(|&i| d[i] <= d[i - 1] && d[i] < d[i + 1]).map(|i| (i, d[i]));
                out.farthest = (1..d.len().saturating_sub(1)).find(|&i| d[i] >= d[i - 1] && d[i] > d[i + 1]).map(|i| (i, d[i]));
                out.returns = out.farthest.is_some_and(|(i, far)| {
                    let near = d[..i].iter().copied().fold(f64::MAX, f64::min);
                    d[i..].iter().any(|x| *x <= 0.5 * (near + far))
                });
            }
            if job.held {
                out.held = relative(job, &self.held.path);
            }
        }
        if let Some(shell) = &self.shell {
            out.shell = relative(job, &shell.path);
        }
        Some(out)
    }
}

#[cfg(target_arch = "wasm32")]
impl Predictor {
    pub fn new() -> Self {
        Self { doing: None, kept: None, latest: Paths::default() }
    }

    /// Go on with the prediction under way for the `spare` seconds this frame has (and for a
    /// millisecond at least: it has to get done). True when a new job may be handed in.
    pub fn poll(&mut self, spare: f64) -> bool {
        if let Some(doing) = &mut self.doing {
            if let Some(paths) = doing.go(&mut self.kept, crate::fmt::now() + spare.clamp(0.001, 0.012)) {
                (self.latest, self.doing) = (paths, None);
            }
        }
        self.doing.is_none()
    }

    pub fn submit(&mut self, job: Job) {
        self.doing = Some(Doing::new(job));
    }

    pub fn clear(&mut self) {
        self.latest = Paths::default();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Predictor {
    pub fn new() -> Self {
        let (tx, job_rx) = channel::<Job>();
        let (res_tx, rx) = channel::<Paths>();
        std::thread::Builder::new()
            .name("gsim-predictor".into())
            .spawn(move || {
                let mut kept = None;
                while let Ok(job) = job_rx.recv() {
                    let paths = Doing::new(job).go(&mut kept, f64::INFINITY).expect("nothing to stop for");
                    if res_tx.send(paths).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn predictor thread");
        Self { tx, rx, busy: false, latest: Paths::default() }
    }

    /// Collect a finished result, if any. Returns true when the worker is free for a new job.
    /// (What the frame has to `spare` is nothing to a worker with a thread of its own.)
    pub fn poll(&mut self, _spare: f64) -> bool {
        match self.rx.try_recv() {
            Ok(paths) => {
                self.latest = paths;
                self.busy = false;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.busy = true,
        }
        !self.busy
    }

    pub fn submit(&mut self, job: Job) {
        self.busy = self.tx.send(job).is_ok();
    }

    pub fn clear(&mut self) {
        self.latest = Paths::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsim_client_core::eph::Eph;
    use gsim_core::{Body, MassiveState};

    #[test]
    fn reference_frame_follows_a_body_that_merges() {
        let rules = GameRules::new(86400.0, 60);
        let body = |x: f64, mass: f64, radius: f64| Body { x, y: 5.0e10, vx: 0.0, vy: 0.0, mass, radius };
        // Slot 0 (light) falls into slot 1 (heavy) within a few ticks, far from the origin.
        let state = MassiveState::from_bodies(&[body(1.0e11, 4.0e25, 1.2e7), body(1.0e11 + 5.0e7, 5.0e25, 1.3e7)]);
        let mut eph = Eph::new(&state.snapshot(), &rules, true);
        eph.request(40, 0);
        let reader = eph.reader();
        while reader.get(39).is_none() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let merged_at = (0..39).find(|t| !reader.get(*t).unwrap().merges.is_empty()).expect("the pair merges");
        assert!(merged_at > 0 && merged_at < 30);

        // A path that sits still at a fixed point, measured relative to the light body.
        let fixed = (1.0e11, 6.0e10);
        let path = Path { start_tick: 0, points: vec![fixed; 40], impact: None };
        let job = Job {
            reader,
            rules,
            ship: ShipState::default(),
            start: 0,
            timeline: InputTimeline::new(),
            ticks: 40,
            show_ship: true,
            held: false,
            burn_mmps: 0,
            shell: None,
            ref_slot: Some(0),
        };
        let rel = relative(&job, &path);
        // Before the fix the dead slot read as the origin, a jump of ~1e11 m at the merge.
        for pair in rel.windows(2) {
            let jump = ((pair[1].0 - pair[0].0).powi(2) + (pair[1].1 - pair[0].1).powi(2)).sqrt();
            assert!(jump < 1.0e8, "relative path jumped by {jump:e} m");
        }
        let end = rel.last().unwrap();
        assert!(end.0.abs() < 1.0e8 && (end.1 - 1.0e10).abs() < 1.0e8, "still measured from the merged body: {end:?}");
    }
}
