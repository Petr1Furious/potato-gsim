//! Transport for [`crate::Session`]: UDP for a program, a WebSocket for a page in a browser
//! (which can open nothing else). Both carry the same messages.

#[cfg(not(target_arch = "wasm32"))]
mod udp;
#[cfg(not(target_arch = "wasm32"))]
pub use udp::{resolve, NetClient};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::NetClient;
