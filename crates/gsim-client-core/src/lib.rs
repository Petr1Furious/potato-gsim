//! Headless client: clock sync, local ephemeris, per-entity replicas with rollback, and
//! input scheduling. No rendering and no sockets in [`Session`], so it runs identically
//! in tests, in the bot and in the GUI.

pub mod clock;
pub mod eph;
pub mod net;
pub mod session;
pub mod world;

pub use session::{Controls, Session, SessionConfig, Stats};
pub use world::{Effect, EffectKind, World};
