//! Wire protocol. The server never streams positions: it sends a snapshot once, then only
//! tick-stamped inputs and events. Clients reproduce everything else themselves.

use gsim_core::{GameRules, MassiveSnapshot, Particle, ShipInput, ShipState, Tick};
use serde::{Deserialize, Serialize};

/// Bump on any wire or simulation change.
pub const PROTOCOL_VERSION: u32 = 4;
pub const DEFAULT_PORT: u16 = 27777;
pub const MAX_NAME_CHARS: usize = 24;
/// Commands are re-sent until acknowledged; this bounds one packet.
pub const MAX_CMDS_PER_PACKET: usize = 48;

pub type PlayerId = u32;
pub type ShellId = u32;

pub mod identity;
pub use identity::Identity;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CmdKind {
    /// "From `tick` on, my controls are this."
    Set(ShipInput),
    /// Launch a shell at `tick`; `speed` is relative to the ship (m/s), clamped by the server.
    Fire { angle: u16, speed: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cmd {
    /// Per-client, starts at 1, strictly increasing.
    pub seq: u32,
    /// Tick the client wants this applied at (its present plus the input delay).
    pub tick: Tick,
    pub kind: CmdKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Reliable. `golden` proves the client's floating point matches (see `gsim_core::selftest`);
    /// `key` is the player's public identity key, which the server then challenges.
    Hello { protocol: u32, golden: u64, name: String, key: [u8; 32] },
    /// Reliable: signature over the server's challenge (see [`identity`]).
    Auth { signature: Vec<u8> },
    /// Unreliable; carries every command not yet acknowledged, oldest first.
    Cmds(Vec<Cmd>),
    /// Unreliable clock probe.
    Ping { client_time: f64 },
    /// Reliable: my massive-tier hash disagreed, send a fresh snapshot.
    ResyncRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub name: String,
    pub kills: u32,
    pub deaths: u32,
    /// Objectives captured this round.
    pub captures: u32,
    pub ship: Option<ShipState>,
    /// Current and already-scheduled future input changes.
    pub inputs: Vec<(Tick, ShipInput)>,
    pub respawn_tick: Option<Tick>,
    pub next_fire_tick: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellInfo {
    pub id: ShellId,
    pub owner: PlayerId,
    pub spawn_tick: Tick,
    pub p: Particle,
}

/// Complete world at the start of `massive.tick`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    pub your_id: PlayerId,
    pub preset: String,
    pub round: u32,
    /// Tick at which the current round ends (`None`: endless).
    pub round_end_tick: Option<Tick>,
    /// Set between rounds: the tick at which the next world starts.
    pub next_round_tick: Option<Tick>,
    /// Body slot to orbit for points.
    pub target: Option<u32>,
    pub rules: GameRules,
    pub massive: MassiveSnapshot,
    pub names: Vec<(u32, String)>,
    pub players: Vec<PlayerInfo>,
    pub shells: Vec<ShellInfo>,
}

/// Reliable, ordered, tick-stamped facts. A `tick` is the first tick at which the fact holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    PlayerJoined { id: PlayerId, name: String },
    PlayerLeft { id: PlayerId },
    ShipSpawn { tick: Tick, player: PlayerId, state: ShipState },
    Input { player: PlayerId, tick: Tick, input: ShipInput },
    ShellSpawn { tick: Tick, id: ShellId, owner: PlayerId, p: Particle },
    ShellGone { tick: Tick, id: ShellId, exploded: bool },
    ShipDied { tick: Tick, player: PlayerId, killer: Option<PlayerId>, body: Option<u32>, respawn_tick: Tick },
    /// The objective moved to another body (or there is none).
    Objective { tick: Tick, target: Option<u32> },
    /// `player` held an orbit around `target` long enough.
    Captured { tick: Tick, player: PlayerId, target: u32 },
    /// Scores are final; a new world starts at `next_round_tick`.
    RoundOver { tick: Tick, next_round_tick: Tick },
    /// Massive-tier hash at the start of `tick`.
    Hash { tick: Tick, hash: u64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Reject { reason: String },
    /// Reliable: prove you hold the key you announced by signing this.
    Challenge { nonce: [u8; 32] },
    /// Sent on join and again on resync.
    Welcome(Box<Welcome>),
    /// `server_tick` is fractional: tick about to be simulated plus progress towards it.
    Pong { client_time: f64, server_tick: f64 },
    /// Reliable, to the sender only: the tick the command really took effect at.
    CmdAck { seq: u32, tick: Tick, accepted: bool },
    Event(Event),
    /// Unreliable, a few times a second: ticks each player has held the objective orbit.
    Progress { holds: Vec<(PlayerId, u32)> },
    /// Unreliable safety net: authoritative ship state at the start of `tick`.
    ShipCheck { tick: Tick, player: PlayerId, state: ShipState },
}

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    postcard::to_allocvec(msg).expect("message serialisation cannot fail")
}

pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Option<T> {
    postcard::from_bytes(bytes).ok()
}

/// Channel ids shared by both directions.
pub const CH_RELIABLE: u8 = 0;
pub const CH_UNRELIABLE: u8 = 1;

impl ClientMsg {
    pub fn reliable(&self) -> bool {
        matches!(self, ClientMsg::Hello { .. } | ClientMsg::Auth { .. } | ClientMsg::ResyncRequest)
    }
}

impl ServerMsg {
    pub fn reliable(&self) -> bool {
        !matches!(self, ServerMsg::Pong { .. } | ServerMsg::ShipCheck { .. } | ServerMsg::Progress { .. })
    }
}

/// Transport channel layout, identical on both ends.
pub fn connection_config() -> renet::ConnectionConfig {
    use renet::{ChannelConfig, SendType};
    use std::time::Duration;
    let channels = vec![
        ChannelConfig {
            channel_id: CH_RELIABLE,
            // A join snapshot of a few thousand bodies must fit.
            max_memory_usage_bytes: 16 * 1024 * 1024,
            send_type: SendType::ReliableOrdered { resend_time: Duration::from_millis(100) },
        },
        ChannelConfig {
            channel_id: CH_UNRELIABLE,
            max_memory_usage_bytes: 2 * 1024 * 1024,
            send_type: SendType::Unreliable,
        },
    ];
    renet::ConnectionConfig {
        available_bytes_per_tick: 256 * 1024,
        server_channels_config: channels.clone(),
        client_channels_config: channels,
    }
}

/// Netcode protocol id: ties the handshake to this protocol version.
pub fn netcode_protocol_id() -> u64 {
    0x6773_696d_0000_0000 | PROTOCOL_VERSION as u64
}

/// The form of a player name both sides agree on: trimmed, printable, bounded.
pub fn clean_name(name: &str) -> String {
    name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect::<String>().trim().to_string()
}
