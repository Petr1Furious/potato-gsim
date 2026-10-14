//! What the exact engine costs: `cargo run --release -p gsim-server --example exact`.
//!
//! A tick of the bodies (what a server and every client work out sixty times a second), a
//! tick of one ship or shell among them, and a prediction of a ship's path.

use gsim_core::particle::{step_particle, Particle, Scratch};
use gsim_core::predict::predict_ship;
use gsim_core::ship::{ShipInput, ShipState};
use gsim_core::{EphProducer, EphRow, GameRules, MassiveState};
use gsim_server::scenario::{self, Params};
use std::sync::Arc;
use std::time::Instant;

fn world(count: usize) -> MassiveState {
    let params = Params::from([("count".to_string(), count as f64)]);
    MassiveState::from_bodies(&scenario::build("random", 7, &params).unwrap().bodies)
}

/// Milliseconds a tick of `count` bodies takes on `threads` threads.
fn tick_ms(count: usize, threads: usize) -> f64 {
    let rules = GameRules::new(86400.0, 60);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    pool.install(|| {
        let mut state = world(count);
        let ticks = (40_000_000 / (count * count)).clamp(20, 2000);
        state.step(&rules);
        let t0 = Instant::now();
        for _ in 0..ticks {
            state.step(&rules);
        }
        t0.elapsed().as_secs_f64() * 1e3 / ticks as f64
    })
}

fn main() {
    let all = std::thread::available_parallelism().map_or(1, |n| n.get());
    println!("vectors: {}", gsim_core::wide::kind());
    println!("a tick of the bodies (ms); on one thread, also the share of a core that is at 60 ticks a second");
    println!("{:>7} | {:>9} {:>7} | {:>10}", "bodies", "1 thread", "share", format!("{all} threads"));
    for count in [250, 500, 1000, 2000, 3000, 5000] {
        let (one, many) = (tick_ms(count, 1), tick_ms(count, all));
        println!("{count:>7} | {one:>9.3} {:>6.1}% | {many:>10.3}", one * 6.0);
    }

    let rules = GameRules::new(86400.0, 60);
    println!("\none ship or shell among the bodies, and a ship's path 20 s ahead (1 thread)");
    println!("{:>7} | {:>12} | {:>12}", "bodies", "a tick (us)", "a path (ms)");
    for count in [250, 1000, 5000] {
        let state = world(count);
        let ticks = 1200;
        let mut producer = EphProducer::new(&state.snapshot(), rules.clone());
        let rows: Vec<Arc<EphRow>> = (0..ticks).map(|_| Arc::new(producer.next_row())).collect();
        let start = Particle { x: 3.0e10, y: 1.0e10, vx: 0.0, vy: 2.0e4 };
        let (mut p, mut scratch) = (start, Scratch::default());
        let t0 = Instant::now();
        for row in &rows {
            step_particle(&mut p, (0.0, 0.0), 10.0, &row.view(), &rules, &mut scratch);
        }
        let each = t0.elapsed().as_secs_f64() * 1e6 / ticks as f64;
        let t0 = Instant::now();
        let path = predict_ship(ShipState::new(start, &rules), 0, ticks as u32, |_, _| ShipInput::default(), |t| rows.get(t as usize).cloned(), &rules);
        println!("{count:>7} | {each:>12.1} | {:>12.2}   ({} points)", t0.elapsed().as_secs_f64() * 1e3, path.points.len());
    }
}
