//! Per-tick record of the massive tier. Test particles (ships, shells) and trajectory
//! prediction integrate against these rows, so the massive bodies themselves never have to
//! be re-simulated when a ship is rolled back.

use crate::{GameRules, MassiveSnapshot, MassiveState, MassiveView, MergeEvent, Tick};
use std::collections::VecDeque;
use std::sync::Arc;

/// Properties that only change when bodies merge; shared between consecutive rows.
#[derive(Clone, Debug)]
pub struct BodyProps {
    pub mass: Vec<f64>,
    pub radius: Vec<f64>,
    pub alive: Vec<bool>,
}

/// Start-of-tick state of every massive body.
#[derive(Clone, Debug)]
pub struct EphRow {
    pub tick: Tick,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub vx: Vec<f64>,
    pub vy: Vec<f64>,
    pub ax: Vec<f64>,
    pub ay: Vec<f64>,
    pub props: Arc<BodyProps>,
    /// [`MassiveState::hash`] at the start of this tick.
    pub hash: u64,
    /// Merges that happen at the end of this tick (visible in the next row).
    pub merges: Vec<MergeEvent>,
}

impl EphRow {
    pub fn view(&self) -> MassiveView<'_> {
        MassiveView {
            x: &self.x,
            y: &self.y,
            vx: &self.vx,
            vy: &self.vy,
            ax: &self.ax,
            ay: &self.ay,
            mass: &self.props.mass,
            radius: &self.props.radius,
            alive: &self.props.alive,
        }
    }
}

/// Owns the massive state and emits one row per tick.
pub struct EphProducer {
    state: MassiveState,
    rules: GameRules,
    props: Arc<BodyProps>,
}

impl EphProducer {
    pub fn new(snapshot: &MassiveSnapshot, rules: GameRules) -> Self {
        let state = MassiveState::from_snapshot(snapshot);
        let props = Arc::new(BodyProps {
            mass: state.mass.clone(),
            radius: state.radius.clone(),
            alive: state.alive.clone(),
        });
        Self { state, rules, props }
    }

    /// Tick of the row the next call to [`Self::next_row`] will return.
    pub fn next_tick(&self) -> Tick {
        self.state.tick
    }

    pub fn next_row(&mut self) -> EphRow {
        self.state.ensure_acc(&self.rules);
        let s = &self.state;
        let mut row = EphRow {
            tick: s.tick,
            x: s.x.clone(),
            y: s.y.clone(),
            vx: s.vx.clone(),
            vy: s.vy.clone(),
            ax: s.ax.clone(),
            ay: s.ay.clone(),
            props: self.props.clone(),
            hash: s.hash(),
            merges: Vec::new(),
        };
        row.merges = self.state.step(&self.rules);
        if !row.merges.is_empty() {
            self.props = Arc::new(BodyProps {
                mass: self.state.mass.clone(),
                radius: self.state.radius.clone(),
                alive: self.state.alive.clone(),
            });
        }
        row
    }
}

/// Contiguous window of rows.
#[derive(Default)]
pub struct EphRing {
    rows: VecDeque<Arc<EphRow>>,
}

impl EphRing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.rows.clear();
    }

    pub fn push(&mut self, row: EphRow) {
        debug_assert!(self.rows.back().is_none_or(|b| b.tick + 1 == row.tick));
        self.rows.push_back(Arc::new(row));
    }

    pub fn get(&self, tick: Tick) -> Option<Arc<EphRow>> {
        let first = self.rows.front()?.tick;
        if tick < first {
            return None;
        }
        self.rows.get((tick - first) as usize).cloned()
    }

    pub fn first_tick(&self) -> Option<Tick> {
        self.rows.front().map(|r| r.tick)
    }

    /// One past the newest row.
    pub fn end_tick(&self) -> Option<Tick> {
        self.rows.back().map(|r| r.tick + 1)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn trim_before(&mut self, tick: Tick) {
        while self.rows.front().is_some_and(|r| r.tick < tick) {
            self.rows.pop_front();
        }
    }
}
