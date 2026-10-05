//! Authoritative game server. [`Authority`] is transport-agnostic (tests drive it over a
//! simulated link); [`net`] runs it on real UDP.

pub mod authority;
pub mod net;
pub mod rng;
pub mod scenario;
pub mod state;

pub use authority::{Authority, ConnId, Outgoing, Target};
pub use scenario::{RandomOpts, Scenario};
