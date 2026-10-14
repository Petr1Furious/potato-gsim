//! How many bodies can this machine step in real time?

use crate::runner::Pacer;
use crate::scenario::{self, Params};
use crate::Sim;
use std::time::Instant;

/// How long a step may take (ms). Bodies are drawn part of the way along the step under way,
/// so they move smoothly however long it takes; what matters is that the world keeps its pace
/// and still answers an edit quickly. Twenty steps a second do both.
pub const STEP_BUDGET_MS: f64 = 50.0;

/// How long each measurement plays a world for (s), after a first step to learn roughly what
/// a step costs. The steps are then as long as the game would make them from the start.
const PLAY: f64 = 1.0;

/// Milliseconds a step of `scenario` with `count` bodies takes, played the way the game
/// plays it and averaged over the second half of [`PLAY`] seconds; stretched by however much
/// the world falls behind its pace (the game slows time down when the close pairs' own steps
/// cost too much, rather than letting the steps grow).
pub fn step_ms(name: &str, params: &Params, count: usize) -> Result<f64, String> {
    let mut params = params.clone();
    params.insert("count".into(), count as f64);
    let setup = scenario::build(name, 1, &params)?;
    let pace = setup.time_scale;
    let mut sim = Sim::new(setup);
    // One short step, to start the pacer where the game's would end up after a while.
    let mut pacer = Pacer::default();
    let started = Instant::now();
    sim.step(pacer.next(pace, sim.engine.longest_step()));
    pacer = Pacer::warmed(started.elapsed().as_secs_f64());
    let (mut played, mut late, mut steps, mut covered) = (0.0, 0.0, 0, 0.0);
    while played < PLAY {
        let started = Instant::now();
        let dt = pacer.next(pace, sim.engine.longest_step());
        sim.step(dt);
        let took = started.elapsed().as_secs_f64();
        pacer.took(dt, took, sim.engine.stats.own_ms() as f64 * 1.0e-3);
        played += took;
        if played > 0.5 * PLAY {
            (late, steps, covered) = (late + took, steps + 1, covered + dt);
        }
    }
    let kept = (covered / (pace * late)).clamp(0.05, 1.0);
    Ok(1.0e3 * late / steps.max(1) as f64 / kept)
}

/// A body count whose steps take about `budget_ms`.
///
/// A step costs a little more than in proportion to the bodies (more of them share the same
/// space and pair up closely), so the cost is taken to grow as a power of the count: guessed
/// at first, then fitted to the measurements. Two or three measurements settle it.
pub fn suggest(name: &str, params: &Params, budget_ms: f64) -> Result<usize, String> {
    // (bodies, ms a step), for the largest count seen to fit and the smallest seen not to.
    type Measured = Option<(f64, f64)>;
    let (mut fits, mut too_many): (Measured, Measured) = (None, None);
    let mut count = 100_000.0f64;
    let mut last: Option<(f64, f64)> = None;
    for _ in 0..3 {
        let ms = step_ms(name, params, count as usize)?;
        if ms <= budget_ms {
            fits = fits.filter(|f| f.0 > count).or(Some((count, ms)));
        } else {
            too_many = too_many.filter(|t| t.0 < count).or(Some((count, ms)));
        }
        // How steeply the cost grows with the count: from the last two measurements, else a guess.
        let power = last.map_or(1.15, |(n, t): (f64, f64)| ((ms / t).ln() / (count / n).ln()).clamp(1.0, 2.0));
        let next = count * (budget_ms / ms).powf(1.0 / power);
        // Never past what was seen to be too many, never short of what was seen to fit.
        let next = next.min(too_many.map_or(f64::MAX, |t| t.0)).max(fits.map_or(0.0, |f| f.0));
        // Round to two significant digits: nobody wants 437,912 bodies.
        let unit = 10f64.powf(next.log10().floor() - 1.0);
        let next = ((next / unit).floor() * unit).clamp(2_000.0, 4_000_000.0);
        let settled = (ms / budget_ms - 1.0).abs() < 0.1 || (next / count - 1.0).abs() < 0.1;
        last = Some((count, ms));
        count = next;
        if settled {
            break;
        }
    }
    // The last estimate, unless it was never borne out and something smaller was.
    Ok(match (fits, too_many) {
        (Some(a), Some(b)) if count > a.0 && count < b.0 => count,
        (Some(a), _) if count < a.0 => a.0,
        _ => count,
    } as usize)
}
