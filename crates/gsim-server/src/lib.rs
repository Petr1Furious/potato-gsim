//! Authoritative game server. [`Authority`] is transport-agnostic (tests drive it over a
//! simulated link); [`net`] runs it on real UDP, and over WebSockets ([`web`]) for browsers.

pub mod authority;
#[cfg(not(target_arch = "wasm32"))]
pub mod net;
pub mod rng;
pub mod scenario;
pub mod state;
#[cfg(not(target_arch = "wasm32"))]
pub mod web;

pub use authority::{Authority, ConnId, Outgoing, Target};
pub use scenario::{Params, Scenario};
