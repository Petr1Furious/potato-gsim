//! Fast approximate gravity for very large worlds (single player only).
//!
//! Unlike `gsim-core` nothing here is reproducible between machines, or even between runs:
//! forces are single precision, summed in whatever order the threads finish, with whatever
//! vector instructions the CPU has. In exchange it steps hundreds of thousands of bodies
//! sixty times a second.

#![allow(clippy::too_many_arguments)]

mod kernel;
mod simd;

pub mod bench;
pub mod engine;
pub mod runner;
pub mod scenario;
pub mod sim;

pub use engine::{Bodies, Engine, Local, Merge, StepStats};
pub use kernel::Level;
pub use sim::Sim;
