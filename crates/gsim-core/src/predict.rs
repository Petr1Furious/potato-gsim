//! Trajectory prediction. It calls the very same step functions as the live simulation
//! against the already-computed ephemeris, so the predicted path is what will actually
//! happen as long as inputs match the ones assumed here.

use crate::particle::{step_particle, Particle, Scratch};
use crate::ship::{step_ship, ShipInput, ShipState};
use crate::{EphRow, GameRules, Tick};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct Path {
    /// Tick of `points[0]`; consecutive points are one tick apart.
    pub start_tick: Tick,
    pub points: Vec<(f64, f64)>,
    /// Body slot hit at the last point, if the path ends in a crash.
    pub impact: Option<u32>,
}

/// Predict a ship from `start_tick` for up to `max_ticks`, or until the ephemeris runs out.
pub fn predict_ship(
    start: ShipState,
    start_tick: Tick,
    max_ticks: u32,
    input_at: impl Fn(Tick) -> ShipInput,
    row_at: impl Fn(Tick) -> Option<Arc<EphRow>>,
    rules: &GameRules,
) -> Path {
    let mut s = start;
    let mut scratch = Scratch::default();
    let mut path = Path { start_tick, points: vec![(s.p.x, s.p.y)], impact: None };
    for k in 0..max_ticks as Tick {
        let t = start_tick + k;
        let Some(row) = row_at(t) else { break };
        let hit = step_ship(&mut s, input_at(t), &row.view(), rules, &mut scratch);
        path.points.push((s.p.x, s.p.y));
        if hit.is_some() {
            path.impact = hit;
            break;
        }
    }
    path
}

/// Predict a ballistic particle (e.g. a shell about to be fired).
pub fn predict_particle(
    start: Particle,
    radius: f64,
    start_tick: Tick,
    max_ticks: u32,
    row_at: impl Fn(Tick) -> Option<Arc<EphRow>>,
    rules: &GameRules,
) -> Path {
    let mut p = start;
    let mut scratch = Scratch::default();
    let mut path = Path { start_tick, points: vec![(p.x, p.y)], impact: None };
    for k in 0..max_ticks as Tick {
        let Some(row) = row_at(start_tick + k) else { break };
        let hit = step_particle(&mut p, (0.0, 0.0), radius, &row.view(), rules, &mut scratch);
        path.points.push((p.x, p.y));
        if hit.is_some() {
            path.impact = hit;
            break;
        }
    }
    path
}
