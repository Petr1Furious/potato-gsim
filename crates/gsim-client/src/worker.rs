//! In a browser: a worker beside the page (`web/worker.js`) that works out the bodies' future
//! and the ship's predicted paths, so that the page's own thread only draws, takes the
//! player's input and steps their ship.
//!
//! The worker runs this same program, never started as a game: it is handed the world once
//! (a snapshot of the bodies, from which it works out exactly what the page would have) and
//! is then asked for rows up to a tick, and for predictions. Both sides talk in messages, with
//! no memory in common.

use crate::predictor::{Coast, Doing, Job, Paths};
use gsim_client_core::eph::{Eph, Feed, Source};
use gsim_core::{BodyProps, EphRow, GameRules, MassiveSnapshot, MergeEvent, Particle, ShipInput, ShipState, Tick};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::Arc;

/// What the bodies are made of, as it travels.
#[derive(Serialize, Deserialize)]
struct Props {
    mass: Vec<f64>,
    radius: Vec<f64>,
    alive: Vec<bool>,
    pull: Vec<f64>,
    fixed: Vec<bool>,
}

/// A row of the ephemeris as it travels: what the bodies are made of only when that changed.
#[derive(Serialize, Deserialize)]
struct Row {
    tick: Tick,
    x: Vec<f64>,
    y: Vec<f64>,
    vx: Vec<f64>,
    vy: Vec<f64>,
    ax: Vec<f64>,
    ay: Vec<f64>,
    props: Option<Props>,
    hash: u64,
    merges: Vec<MergeEvent>,
}

/// A prediction to make, as it travels.
#[derive(Serialize, Deserialize)]
struct Asked {
    ship: ShipState,
    start: Tick,
    timeline: Vec<(Tick, ShipInput)>,
    ticks: u32,
    show_ship: bool,
    held: bool,
    burn_mmps: i64,
    shell: Option<Particle>,
    ref_slot: Option<u32>,
}

/// From the page. `world` numbers the worlds, so that what belongs to an earlier one is
/// told apart.
#[derive(Serialize, Deserialize)]
enum ToWorker {
    World { world: u32, snapshot: Box<MassiveSnapshot>, rules: Box<GameRules> },
    Want { world: u32, upto: Tick, keep: Tick },
    Predict { world: u32, asked: Asked },
}

#[derive(Serialize, Deserialize)]
enum FromWorker {
    Rows { world: u32, rows: Vec<Row> },
    Paths { world: u32, paths: Paths },
    /// A prediction not made: a newer one was asked for before it was begun.
    Skipped,
}

// --- the page's side ---------------------------------------------------------------------------

extern "C" {
    /// Start the worker. False if the browser cannot.
    fn gsim_worker_start() -> i32;
    /// 0 while it loads, 1 once it is ready, 2 if it failed.
    fn gsim_worker_state() -> i32;
    fn gsim_worker_send(ptr: *const u8, len: usize);
    /// Length of the next message from it, or -1; `gsim_worker_take` then copies it out.
    fn gsim_worker_next() -> i32;
    fn gsim_worker_take(ptr: *mut u8);
}

/// What has come back from the worker.
#[derive(Default)]
struct Link {
    world: u32,
    rows: Vec<EphRow>,
    /// An answer for each prediction asked for: what it came to, if it is for this world.
    answers: Vec<Option<Paths>>,
    props: Option<Arc<BodyProps>>,
}

thread_local! {
    static LINK: RefCell<Link> = RefCell::default();
}

/// Start the worker, as early as possible: it has the program to load.
pub fn start() {
    // SAFETY (for the calls into the page's script here): it reads and writes only the
    // bytes it is given the place and length of.
    unsafe { gsim_worker_start() };
}

/// The worker works for us (it may still be loading).
pub fn working() -> bool {
    unsafe { gsim_worker_state() != 2 }
}

fn send(message: &ToWorker) {
    let bytes = gsim_proto::encode(message);
    unsafe { gsim_worker_send(bytes.as_ptr(), bytes.len()) };
}

/// Take in what the worker has sent.
fn pump(link: &mut Link) {
    loop {
        let len = unsafe { gsim_worker_next() };
        let Ok(len) = usize::try_from(len) else { break };
        let mut bytes = vec![0u8; len];
        unsafe { gsim_worker_take(bytes.as_mut_ptr()) };
        match gsim_proto::decode::<FromWorker>(&bytes) {
            Some(FromWorker::Rows { world, rows }) if world == link.world => {
                for row in rows {
                    let props = match row.props {
                        Some(Props { mass, radius, alive, pull, fixed }) => Arc::new(BodyProps { mass, radius, alive, pull, fixed }),
                        None => match &link.props {
                            Some(props) => props.clone(),
                            None => continue,
                        },
                    };
                    link.props = Some(props.clone());
                    let Row { tick, x, y, vx, vy, ax, ay, hash, merges, .. } = row;
                    link.rows.push(EphRow { tick, x, y, vx, vy, ax, ay, props, hash, merges });
                }
            }
            Some(FromWorker::Paths { world, paths }) => {
                let answer = (world == link.world).then_some(paths);
                link.answers.push(answer);
            }
            Some(FromWorker::Skipped) => link.answers.push(None),
            _ => {}
        }
    }
}

/// The ephemeris of one world, from the worker.
struct FromTheWorker {
    world: u32,
    asked: Tick,
}

impl Feed for FromTheWorker {
    fn want(&mut self, upto: Tick, keep: Tick) {
        if upto != self.asked {
            self.asked = upto;
            send(&ToWorker::Want { world: self.world, upto, keep });
        }
    }

    fn arrived(&mut self) -> Vec<EphRow> {
        LINK.with(|link| {
            let mut link = link.borrow_mut();
            pump(&mut link);
            if link.world == self.world { std::mem::take(&mut link.rows) } else { Vec::new() }
        })
    }
}

/// A world's ephemeris from the worker, if it works.
pub fn feed(snapshot: &MassiveSnapshot, rules: &GameRules) -> Option<Box<dyn Feed>> {
    if !working() {
        return None;
    }
    let world = LINK.with(|link| {
        let mut link = link.borrow_mut();
        // (Answers still to come are answers all the same.)
        let answers = std::mem::take(&mut link.answers);
        *link = Link { world: link.world + 1, answers, ..Link::default() };
        link.world
    });
    send(&ToWorker::World { world, snapshot: Box::new(snapshot.clone()), rules: Box::new(rules.clone()) });
    Some(Box::new(FromTheWorker { world, asked: 0 }))
}

/// Ask the worker for a prediction (for the world it was last handed).
pub fn predict(job: &Job) {
    let world = LINK.with(|link| link.borrow().world);
    let asked = Asked {
        ship: job.ship,
        start: job.start,
        timeline: job.timeline.changes(),
        ticks: job.ticks,
        show_ship: job.show_ship,
        held: job.held,
        burn_mmps: job.burn_mmps,
        shell: job.shell,
        ref_slot: job.ref_slot,
    };
    send(&ToWorker::Predict { world, asked });
}

/// An answer for each prediction that has come back: `None` for one that was not made, or
/// was made for another world.
pub fn answers() -> Vec<Option<Paths>> {
    LINK.with(|link| {
        let mut link = link.borrow_mut();
        pump(&mut link);
        std::mem::take(&mut link.answers)
    })
}

// --- the worker's side --------------------------------------------------------------------------

extern "C" {
    /// Send a message to the page.
    fn gsim_worker_post(ptr: *const u8, len: usize);
}

/// Rows sent in one message at most.
const BATCH: usize = 30;

struct Worker {
    world: u32,
    eph: Eph,
    rules: GameRules,
    /// The first row not sent yet, the one after the last wanted, and the first still needed.
    sent: Tick,
    want: Tick,
    keep: Tick,
    /// What the rows sent last were made of.
    props: Option<Arc<BodyProps>>,
    kept: Option<Coast>,
}

thread_local! {
    /// The newest prediction asked for and not begun yet.
    static ASKED: RefCell<Option<(u32, Asked)>> = const { RefCell::new(None) };
}

thread_local! {
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
    static INBOX: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn post(message: &FromWorker) {
    let bytes = gsim_proto::encode(message);
    // SAFETY: the worker's script copies the bytes during the call.
    unsafe { gsim_worker_post(bytes.as_ptr(), bytes.len()) };
}

/// Room for a message of `len` bytes from the page, to be written by the worker's script.
#[no_mangle]
pub extern "C" fn gsim_worker_inbox(len: usize) -> *mut u8 {
    INBOX.with(|inbox| {
        let mut inbox = inbox.borrow_mut();
        inbox.resize(len, 0);
        inbox.as_mut_ptr()
    })
}

/// Deal with the message written into the inbox.
#[no_mangle]
pub extern "C" fn gsim_worker_message() {
    let message = INBOX.with(|inbox| gsim_proto::decode::<ToWorker>(&inbox.borrow()));
    WORKER.with(|worker| {
        let mut worker = worker.borrow_mut();
        match message {
            Some(ToWorker::World { world, snapshot, rules }) => {
                let eph = Eph::new(&snapshot, &rules, Source::Inline { budget: 1 });
                *worker = Some(Worker { world, eph, rules: *rules, sent: snapshot.tick, want: snapshot.tick, keep: 0, props: None, kept: None });
            }
            Some(ToWorker::Want { world, upto, keep }) => {
                if let Some(w) = worker.as_mut().filter(|w| w.world == world) {
                    (w.want, w.keep) = (upto, keep);
                }
            }
            Some(ToWorker::Predict { world, asked }) => {
                // Only the newest is made: the page would only throw the others away.
                let superseded = ASKED.with(|pending| pending.borrow_mut().replace((world, asked))).is_some();
                if superseded {
                    post(&FromWorker::Skipped);
                }
            }
            None => {}
        }
    });
}

/// Make the newest prediction asked for, if there is one.
#[no_mangle]
pub extern "C" fn gsim_worker_predict() {
    let Some((world, asked)) = ASKED.with(|pending| pending.borrow_mut().take()) else { return };
    let paths = WORKER.with(|worker| {
        let mut worker = worker.borrow_mut();
        let w = worker.as_mut().filter(|w| w.world == world)?;
        let job = Job {
            reader: w.eph.reader(),
            rules: w.rules.clone(),
            ship: asked.ship,
            start: asked.start,
            timeline: gsim_core::InputTimeline::from_changes(&asked.timeline),
            ticks: asked.ticks,
            show_ship: asked.show_ship,
            held: asked.held,
            burn_mmps: asked.burn_mmps,
            shell: asked.shell,
            ref_slot: asked.ref_slot,
        };
        Doing::new(job).go(&mut w.kept, f64::INFINITY)
    });
    post(&FromWorker::Paths { world, paths: paths.unwrap_or_default() });
}

/// Work out rows for up to `ms` milliseconds and send them. True if there is more to do.
#[no_mangle]
pub extern "C" fn gsim_worker_work(ms: f64) -> bool {
    let until = crate::fmt::now() + ms * 1e-3;
    WORKER.with(|worker| {
        let mut worker = worker.borrow_mut();
        let Some(w) = worker.as_mut() else { return false };
        while w.eph.end_tick().unwrap_or(w.sent) < w.want && crate::fmt::now() < until {
            w.eph.request(w.want, 1);
        }
        let end = w.eph.end_tick().unwrap_or(w.sent);
        while w.sent < end {
            let mut rows = Vec::new();
            while w.sent < end && rows.len() < BATCH {
                let Some(row) = w.eph.get(w.sent) else { break };
                // What the bodies are made of only goes with the rows where it changed.
                let changed = w.props.as_ref().is_none_or(|props| !Arc::ptr_eq(props, &row.props));
                w.props = Some(row.props.clone());
                let p = &row.props;
                let props = changed.then(|| Props { mass: p.mass.clone(), radius: p.radius.clone(), alive: p.alive.clone(), pull: p.pull.clone(), fixed: p.fixed.clone() });
                let r = &*row;
                rows.push(Row { tick: r.tick, x: r.x.clone(), y: r.y.clone(), vx: r.vx.clone(), vy: r.vy.clone(), ax: r.ax.clone(), ay: r.ay.clone(), props, hash: r.hash, merges: r.merges.clone() });
                w.sent += 1;
            }
            if rows.is_empty() {
                break;
            }
            post(&FromWorker::Rows { world: w.world, rows });
        }
        w.eph.trim_before(w.keep);
        w.eph.end_tick().unwrap_or(w.sent) < w.want
    })
}
