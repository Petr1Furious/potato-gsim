//! Deterministic simulation core.
//!
//! Everything in this crate must produce bit-identical results on every supported machine:
//! server and clients evolve the world independently and only exchange inputs and hashes.
//! Rules of the house:
//! - f64 arithmetic is limited to `+ - * /` and `sqrt` (IEEE-exact). Anything else goes
//!   through [`math`] (pure-Rust libm). See `clippy.toml`.
//! - No `HashMap` iteration, no parallel float reductions: what rayon is given is cut into
//!   pieces that do not depend on the number of threads, each worked through serially, and
//!   the pieces are put together in order.
//! - No clocks, no I/O, no randomness.

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "wasm32")))]
compile_error!("gsim-core relies on IEEE-754 double arithmetic without x87 excess precision");

pub mod ephemeris;
pub mod massive;
pub mod math;
pub mod objective;
pub mod particle;
pub mod predict;
pub mod rules;
pub mod selftest;
pub mod ship;
pub mod wide;

pub use ephemeris::{BodyProps, EphProducer, EphRing, EphRow};
pub use massive::{Body, MassiveSnapshot, MassiveState, MassiveView, MergeEvent, SystemFrame};
pub use particle::{Particle, Scratch};
pub use rules::GameRules;
pub use ship::{InputTimeline, Kit, Magazine, ShipInput, ShipState};

/// Global simulation step index. State "at tick T" is the state before step T is integrated.
pub type Tick = u64;
