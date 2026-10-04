//! Estimates the server's current (fractional) tick from ping round trips.

use std::collections::VecDeque;

pub struct TickClock {
    hz: f64,
    /// `server_tick = local_seconds * hz + offset`.
    offset: f64,
    target: f64,
    samples: VecDeque<(f64, f64)>, // (rtt, offset)
    last_update: Option<f64>,
    synced: bool,
}

const WINDOW: usize = 24;
/// Above this error (ticks) the clock jumps instead of slewing.
const SNAP_TICKS: f64 = 30.0;
/// Maximum rate change while slewing.
const SLEW: f64 = 0.03;

impl TickClock {
    pub fn new(hz: u32, server_tick: f64, now: f64) -> Self {
        let hz = hz as f64;
        let offset = server_tick - now * hz;
        Self { hz, offset, target: offset, samples: VecDeque::new(), last_update: None, synced: false }
    }

    pub fn on_pong(&mut self, client_time: f64, server_tick: f64, now: f64) {
        let rtt = (now - client_time).max(0.0);
        // The server stamped its tick half a round trip ago.
        let sample = server_tick + 0.5 * rtt * self.hz - now * self.hz;
        self.samples.push_back((rtt, sample));
        while self.samples.len() > WINDOW {
            self.samples.pop_front();
        }
        // Trust the fastest round trip: it had the least queueing in either direction.
        let best = self.samples.iter().fold((f64::MAX, sample), |b, s| if s.0 < b.0 { *s } else { b });
        self.target = best.1;
        if !self.synced {
            self.synced = true;
            self.offset = self.target;
        }
    }

    pub fn update(&mut self, now: f64) {
        let dt = self.last_update.map_or(0.0, |l| (now - l).max(0.0));
        self.last_update = Some(now);
        let err = self.target - self.offset;
        if err.abs() > SNAP_TICKS {
            self.offset = self.target;
        } else {
            let max = SLEW * dt * self.hz;
            self.offset += err.clamp(-max, max);
        }
    }

    pub fn server_tick(&self, now: f64) -> f64 {
        now * self.hz + self.offset
    }

    pub fn rtt_min(&self) -> Option<f64> {
        self.samples.iter().map(|s| s.0).fold(None, |m, r| Some(m.map_or(r, |m: f64| m.min(r))))
    }

    pub fn rtt_last(&self) -> Option<f64> {
        self.samples.back().map(|s| s.0)
    }

    pub fn synced(&self) -> bool {
        self.synced
    }
}
