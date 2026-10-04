//! Client-side ephemeris: the massive tier computed locally, ahead of the present.

use gsim_core::{EphProducer, EphRing, EphRow, GameRules, MassiveSnapshot, Tick};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

pub enum Eph {
    /// Rows are produced on demand by the caller (deterministic tests, bots).
    Inline { prod: Box<EphProducer>, ring: EphRing },
    /// A worker thread keeps the ring filled up to `want`.
    Threaded {
        ring: Arc<RwLock<EphRing>>,
        want: Arc<AtomicU64>,
        trim: Arc<AtomicU64>,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    },
}

impl Eph {
    pub fn new(snapshot: &MassiveSnapshot, rules: &GameRules, threaded: bool) -> Self {
        let mut prod = EphProducer::new(snapshot, rules.clone());
        if !threaded {
            return Eph::Inline { prod: Box::new(prod), ring: EphRing::new() };
        }
        let ring = Arc::new(RwLock::new(EphRing::new()));
        let want = Arc::new(AtomicU64::new(snapshot.tick));
        let trim = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (r, w, t, s) = (ring.clone(), want.clone(), trim.clone(), stop.clone());
        let handle = std::thread::Builder::new()
            .name("gsim-ephemeris".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    if prod.next_tick() < w.load(Ordering::Relaxed) {
                        let row = prod.next_row();
                        let mut ring = r.write().unwrap();
                        ring.push(row);
                        ring.trim_before(t.load(Ordering::Relaxed));
                    } else {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            })
            .expect("spawn ephemeris thread");
        Eph::Threaded { ring, want, trim, stop, handle: Some(handle) }
    }

    pub fn get(&self, tick: Tick) -> Option<Arc<EphRow>> {
        match self {
            Eph::Inline { ring, .. } => ring.get(tick),
            Eph::Threaded { ring, .. } => ring.read().unwrap().get(tick),
        }
    }

    /// One past the newest available row (the snapshot tick if nothing is ready yet).
    pub fn end_tick(&self) -> Option<Tick> {
        match self {
            Eph::Inline { ring, .. } => ring.end_tick(),
            Eph::Threaded { ring, .. } => ring.read().unwrap().end_tick(),
        }
    }

    /// Ask for rows up to (excluding) `upto`. Inline mode computes at most `budget` rows now.
    pub fn request(&mut self, upto: Tick, budget: usize) {
        match self {
            Eph::Inline { prod, ring } => {
                let mut n = 0;
                while prod.next_tick() < upto && n < budget {
                    ring.push(prod.next_row());
                    n += 1;
                }
            }
            Eph::Threaded { want, .. } => want.store(upto, Ordering::Relaxed),
        }
    }

    pub fn trim_before(&mut self, tick: Tick) {
        match self {
            Eph::Inline { ring, .. } => ring.trim_before(tick),
            Eph::Threaded { trim, .. } => trim.store(tick, Ordering::Relaxed),
        }
    }
}

impl Drop for Eph {
    fn drop(&mut self) {
        if let Eph::Threaded { stop, handle, .. } = self {
            stop.store(true, Ordering::Relaxed);
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}

/// Cheap, thread-safe read access to a threaded ephemeris (e.g. for a prediction worker).
#[derive(Clone)]
pub struct EphReader {
    ring: Arc<RwLock<EphRing>>,
}

impl EphReader {
    pub fn get(&self, tick: Tick) -> Option<Arc<EphRow>> {
        self.ring.read().unwrap().get(tick)
    }
}

impl Eph {
    /// `None` in inline mode, where the ephemeris lives on the caller's thread.
    pub fn reader(&self) -> Option<EphReader> {
        match self {
            Eph::Inline { .. } => None,
            Eph::Threaded { ring, .. } => Some(EphReader { ring: ring.clone() }),
        }
    }
}
