//! Client-side ephemeris: the massive tier computed locally, ahead of the present.

use gsim_core::{EphProducer, EphRing, EphRow, GameRules, MassiveSnapshot, Tick};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

/// Where the ephemeris is worked out.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// By the caller, at most `budget` rows each time it asks (tests, bots, and a page in a
    /// browser that has nothing better).
    Inline { budget: usize },
    /// On a thread of its own.
    Thread,
    /// Somewhere else entirely, which this hands rows over from (a worker beside a page in a
    /// browser). `None` from it: nowhere after all, and the caller does the work.
    Fed(fn(&MassiveSnapshot, &GameRules) -> Option<Box<dyn Feed>>),
}

/// Rows worked out somewhere else.
pub trait Feed {
    /// The rows up to (not including) `upto` are wanted; those before `keep` no longer are.
    fn want(&mut self, upto: Tick, keep: Tick);
    /// The rows that have arrived since the last call, in order.
    fn arrived(&mut self) -> Vec<EphRow>;
}

pub enum Eph {
    /// Rows are produced on demand by the caller (deterministic tests, bots).
    Inline { prod: Box<EphProducer>, ring: Arc<RwLock<EphRing>> },
    /// Rows come from elsewhere.
    Fed { feed: Box<dyn Feed>, ring: Arc<RwLock<EphRing>>, keep: Tick },

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
    pub fn new(snapshot: &MassiveSnapshot, rules: &GameRules, source: Source) -> Self {
        let feed = if let Source::Fed(start) = source { start(snapshot, rules) } else { None };
        if let Some(feed) = feed {
            return Eph::Fed { feed, ring: Arc::new(RwLock::new(EphRing::new())), keep: 0 };
        }
        let mut prod = EphProducer::new(snapshot, rules.clone());
        if !matches!(source, Source::Thread) {
            return Eph::Inline { prod: Box::new(prod), ring: Arc::new(RwLock::new(EphRing::new())) };
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
            Eph::Inline { ring, .. } | Eph::Fed { ring, .. } | Eph::Threaded { ring, .. } => ring.read().unwrap().get(tick),
        }
    }

    /// One past the newest available row (the snapshot tick if nothing is ready yet).
    pub fn end_tick(&self) -> Option<Tick> {
        match self {
            Eph::Inline { ring, .. } | Eph::Fed { ring, .. } | Eph::Threaded { ring, .. } => ring.read().unwrap().end_tick(),
        }
    }

    /// Ask for rows up to (excluding) `upto`. Inline mode computes at most `budget` rows now.
    pub fn request(&mut self, upto: Tick, budget: usize) {
        match self {
            Eph::Inline { prod, ring } => {
                let mut ring = ring.write().unwrap();
                let mut n = 0;
                while prod.next_tick() < upto && n < budget {
                    ring.push(prod.next_row());
                    n += 1;
                }
            }
            Eph::Fed { feed, ring, keep } => {
                feed.want(upto, *keep);
                let mut ring = ring.write().unwrap();
                for row in feed.arrived() {
                    // (Only what follows on from what is there.)
                    if ring.end_tick().is_none_or(|end| end == row.tick) {
                        ring.push(row);
                    }
                }
            }
            Eph::Threaded { want, .. } => want.store(upto, Ordering::Relaxed),
        }
    }

    pub fn trim_before(&mut self, tick: Tick) {
        match self {
            Eph::Inline { ring, .. } => ring.write().unwrap().trim_before(tick),
            Eph::Fed { ring, keep, .. } => {
                *keep = tick;
                ring.write().unwrap().trim_before(tick);
            }
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

/// Cheap, thread-safe read access to the ephemeris (e.g. for a prediction worker).
#[derive(Clone)]
pub struct EphReader {
    ring: Arc<RwLock<EphRing>>,
}

impl EphReader {
    pub fn get(&self, tick: Tick) -> Option<Arc<EphRow>> {
        self.ring.read().unwrap().get(tick)
    }

    /// Both read the same ephemeris (and not, say, the one from before a resync).
    pub fn same(&self, other: &EphReader) -> bool {
        Arc::ptr_eq(&self.ring, &other.ring)
    }
}

impl Eph {
    pub fn reader(&self) -> EphReader {
        match self {
            Eph::Inline { ring, .. } | Eph::Fed { ring, .. } | Eph::Threaded { ring, .. } => EphReader { ring: ring.clone() },
        }
    }
}
