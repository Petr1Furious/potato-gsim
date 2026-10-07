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

pub struct Predictor {
    tx: Sender<Job>,
    rx: Receiver<Paths>,
    busy: bool,
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

fn compute(job: &Job) -> Paths {
    let t0 = std::time::Instant::now();
    let mut out = Paths { ref_slot: job.ref_slot, ..Default::default() };
    let row_at = |t: Tick| job.reader.get(t);
    if job.show_ship {
        let coast = predict_ship(job.ship, job.start, job.ticks, |_, _| ShipInput::default(), row_at, &job.rules);
        out.coast_impact = coast.impact.is_some();
        out.coast = relative(job, &coast);
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
            // The burn goes on until it has cost so much, and then the ship coasts.
            let until = job.ship.fuel - job.burn_mmps;
            let held = predict_ship(job.ship, job.start, job.ticks, |t, s| if s.fuel > until { job.timeline.at(t) } else { ShipInput::default() }, row_at, &job.rules);
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
        let reader = eph.reader().unwrap();
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
