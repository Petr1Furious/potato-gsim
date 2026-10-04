//! Background trajectory prediction. Exact prediction against a thousand bodies for twenty
//! seconds ahead costs tens of milliseconds, so it runs off the render thread; the newest
//! finished result is drawn, re-anchored to the reference body's current position.

use gsim_client_core::eph::EphReader;
use gsim_core::predict::{predict_particle, predict_ship, Path};
use gsim_core::{GameRules, InputTimeline, Particle, ShipInput, ShipState, Tick};
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
    /// Also predict with the scheduled thrust kept on.
    pub held: bool,
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
    pub compute_ms: f64,
}

pub struct Predictor {
    tx: Sender<Job>,
    rx: Receiver<Paths>,
    busy: bool,
    pub latest: Paths,
}

fn relative(job: &Job, path: &Path) -> Vec<(f64, f64)> {
    let Some(slot) = job.ref_slot.map(|s| s as usize) else { return path.points.clone() };
    let mut last = (0.0, 0.0);
    path.points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let tick = path.start_tick + i as Tick;
            if let Some(row) = job.reader.get(tick) {
                last = (row.x[slot], row.y[slot]);
            } else if let Some(row) = tick.checked_sub(1).and_then(|t| job.reader.get(t)) {
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

fn compute(job: &Job) -> Paths {
    let t0 = std::time::Instant::now();
    let mut out = Paths { ref_slot: job.ref_slot, ..Default::default() };
    let row_at = |t: Tick| job.reader.get(t);
    if job.show_ship {
        let coast = predict_ship(job.ship, job.start, job.ticks, |_| ShipInput::default(), row_at, &job.rules);
        out.coast_impact = coast.impact.is_some();
        out.coast = relative(job, &coast);
        if job.ref_slot.is_some() {
            // The next close approach, not the lowest point anywhere in the window: on a
            // repeating orbit later passes are about as close and would win by a hair.
            let d: Vec<f64> = out.coast.iter().map(|q| (q.0 * q.0 + q.1 * q.1).sqrt()).collect();
            out.closest = (1..d.len().saturating_sub(1)).find(|&i| d[i] <= d[i - 1] && d[i] < d[i + 1]).map(|i| (i, d[i]));
        }
        if job.held {
            let held = predict_ship(job.ship, job.start, job.ticks, |t| job.timeline.at(t), row_at, &job.rules);
            out.held = relative(job, &held);
        }
    }
    if let Some(p) = job.shell {
        let life = job.rules.shell_lifetime_ticks.min(job.ticks);
        let shell = predict_particle(p, job.rules.shell_radius, job.start, life, row_at, &job.rules);
        out.shell = relative(job, &shell);
    }
    out.compute_ms = t0.elapsed().as_secs_f64() * 1e3;
    out
}

impl Predictor {
    pub fn new() -> Self {
        let (tx, job_rx) = channel::<Job>();
        let (res_tx, rx) = channel::<Paths>();
        std::thread::Builder::new()
            .name("gsim-predictor".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    if res_tx.send(compute(&job)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn predictor thread");
        Self { tx, rx, busy: false, latest: Paths::default() }
    }

    /// Collect a finished result, if any. Returns true when the worker is free for a new job.
    pub fn poll(&mut self) -> bool {
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
