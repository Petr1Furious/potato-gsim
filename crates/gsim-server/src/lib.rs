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

/// A line of the server log, as Minecraft writes it: `[12:34:56] [Server thread/INFO]: ...`
/// (the time in UTC).
pub fn log(level: &str, text: &str) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    println!("[{h:02}:{m:02}:{s:02}] [Server thread/{level}]: {text}");
}
