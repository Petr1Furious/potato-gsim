//! How many bodies can this machine step in real time?

use crate::scenario::{self, Params};
use crate::Engine;
use std::time::Instant;

/// Median milliseconds per step of `scenario` with `count` bodies.
pub fn step_ms(name: &str, params: &Params, count: usize, steps: usize) -> Result<f64, String> {
    let mut params = params.clone();
    params.insert("count".into(), count as f64);
    let setup = scenario::build(name, 1, &params)?;
    let mut engine = Engine::new(6.67430e-11, 1.0e6);
    engine.theta = setup.theta;
    let mut bodies = setup.bodies;
    engine.prime(&mut bodies);
    let dt = setup.time_scale / 60.0;
    let mut times: Vec<f64> = (0..steps.max(1))
        .map(|_| {
            let t = Instant::now();
            engine.step(&mut bodies, dt);
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    times.sort_by(f64::total_cmp);
    Ok(times[times.len() / 2])
}

/// A body count that should hold `budget_ms` per step, from two quick measurements.
pub fn suggest(name: &str, params: &Params, budget_ms: f64) -> Result<usize, String> {
    let (small, large) = (40_000, 160_000);
    let (a, b) = (step_ms(name, params, small, 5)?, step_ms(name, params, large, 5)?);
    // Cost is close to linear in the count, with a fixed part.
    let slope = ((b - a) / (large - small) as f64).max(1e-9);
    let fixed = (a - slope * small as f64).max(0.0);
    let count = ((budget_ms - fixed) / slope).clamp(2_000.0, 4_000_000.0);
    // Round to two significant digits: nobody wants 437,912 bodies.
    let unit = 10f64.powf(count.log10().floor() - 1.0);
    Ok(((count / unit).floor() * unit) as usize)
}
