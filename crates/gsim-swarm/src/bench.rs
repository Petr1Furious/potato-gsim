//! How many bodies can this machine step in real time?

use crate::runner::Pacer;
use crate::scenario::{self, Params};
use crate::Sim;
use std::time::Instant;

/// How long each measurement plays a world for (s). Long enough for a disc or a cloud to
/// start clumping: the close pairs that form then are what a step costs most for.
const PLAY: f64 = 8.0;

/// Milliseconds a step of `scenario` with `count` bodies takes, played the way the game
/// plays it and averaged over the second half of [`PLAY`] seconds.
pub fn step_ms(name: &str, params: &Params, count: usize) -> Result<f64, String> {
    let mut params = params.clone();
    params.insert("count".into(), count as f64);
    let setup = scenario::build(name, 1, &params)?;
    let pace = setup.time_scale;
    let mut sim = Sim::new(setup);
    let mut pacer = Pacer::default();
    let (mut played, mut late, mut steps) = (0.0, 0.0, 0);
    while played < PLAY {
        let started = Instant::now();
        let dt = pacer.next(pace, sim.engine.longest_step());
        sim.step(dt);
        let took = started.elapsed().as_secs_f64();
        pacer.took(dt, took, sim.engine.stats.own_ms as f64 * 1.0e-3);
        played += took;
        if played > 0.5 * PLAY {
            (late, steps) = (late + took, steps + 1);
        }
    }
    Ok(1.0e3 * late / steps.max(1) as f64)
}

/// A body count that should hold `budget_ms` per step.
///
/// The cost is not a line through two measurements: the more bodies share the same space,
/// the more of them pair up closely, and those cost the most. So the search keeps the largest
/// count found to fit and the smallest found not to, and measures between them (or, while
/// everything fits, further out by no more than a factor each time).
pub fn suggest(name: &str, params: &Params, budget_ms: f64) -> Result<usize, String> {
    // (bodies, ms a step)
    let (mut fits, mut too_many): (Option<(f64, f64)>, Option<(f64, f64)>) = (None, None);
    let mut count = 60_000.0f64;
    for _ in 0..4 {
        let ms = step_ms(name, params, count as usize)?;
        if ms <= budget_ms {
            fits = Some((count, ms));
        } else {
            too_many = Some((count, ms));
        }
        let next = match (fits, too_many) {
            // Where a line between the two crosses the budget.
            (Some(a), Some(b)) => a.0 + (b.0 - a.0) * (budget_ms - a.1) / (b.1 - a.1),
            // In proportion, but no further than this at once.
            (Some(a), None) => a.0 * (budget_ms / a.1).min(3.0),
            (None, Some(b)) => b.0 * (budget_ms / b.1).max(0.2),
            (None, None) => unreachable!(),
        };
        // Round to two significant digits: nobody wants 437,912 bodies.
        let unit = 10f64.powf(next.log10().floor() - 1.0);
        let next = ((next / unit).floor() * unit).clamp(2_000.0, 4_000_000.0);
        let settled = (next / count - 1.0).abs() < 0.12;
        count = next;
        if settled {
            break;
        }
    }
    // What was seen to fit, unless the last guess lies between that and what did not.
    Ok(match (fits, too_many) {
        (Some(a), Some(b)) if count > a.0 && count < b.0 => count,
        (Some(a), _) => a.0,
        _ => count,
    } as usize)
}
