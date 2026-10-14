//! Wire protocol. The server never streams positions: it sends a snapshot once, then only
//! tick-stamped inputs and events. Clients reproduce everything else themselves.

use gsim_core::{GameRules, Magazine, MassiveSnapshot, Particle, ShipInput, ShipState, Tick};
use serde::{Deserialize, Serialize};

/// Bump on any wire or simulation change.
pub const PROTOCOL_VERSION: u32 = 15;
pub const DEFAULT_PORT: u16 = 27777;
pub const MAX_NAME_CHARS: usize = 24;
/// Commands are re-sent until acknowledged; this bounds one packet.
pub const MAX_CMDS_PER_PACKET: usize = 48;
pub const MAX_CHAT_CHARS: usize = 200;

pub type PlayerId = u32;
pub type ShellId = u32;

pub mod command;
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
    Hello { protocol: u32, golden: u64, name: String, key: [u8; 32], color: [u8; 3] },
    /// Reliable: signature over the server's challenge (see [`identity`]).
    Auth { signature: Vec<u8> },
    /// Unreliable; carries every command not yet acknowledged, oldest first.
    Cmds(Vec<Cmd>),
    /// Unreliable clock probe.
    Ping { client_time: f64 },
    /// Reliable: a chat line; a leading `/` makes it a command (see [`command`]).
    Chat { text: String },
    /// Reliable: draw my ship in this colour from now on.
    Color([u8; 3]),
    /// Reliable: show everyone a marker at this point on the map.
    Mark { x: f64, y: f64 },
    /// Reliable: my massive-tier hash disagreed, send a fresh snapshot.
    ResyncRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub name: String,
    pub color: [u8; 3],
    pub kills: u32,
    pub deaths: u32,
    /// Objectives captured this round.
    pub captures: u32,
    pub ship: Option<ShipState>,
    /// Current and already-scheduled future input changes.
    pub inputs: Vec<(Tick, ShipInput)>,
    pub respawn_tick: Option<Tick>,
    pub next_fire_tick: Tick,
    pub magazine: Magazine,
    /// Watching, with no ship.
    pub spectating: bool,
    /// The ship survives one shell.
    pub shield: bool,
}

/// What a round is about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameMode {
    /// Orbit the marked bodies for points.
    #[default]
    Targets,
    /// Every player has a base; the last one with a ship or a base left wins.
    Bases,
}

impl GameMode {
    pub fn name(self) -> &'static str {
        match self {
            GameMode::Targets => "targets",
            GameMode::Bases => "bases",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        [GameMode::Targets, GameMode::Bases].into_iter().find(|m| m.name().eq_ignore_ascii_case(word))
    }
}

/// A base's health when it is new.
pub const BASE_HP: u32 = 6000;

/// What a repair gives back to a standing base.
pub const REPAIR_HP: u32 = BASE_HP / 5;

/// A player's base: a body that does not move. It is gone (a hulk) at 0 hp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Base {
    pub owner: PlayerId,
    pub slot: u32,
    pub hp: u32,
}

/// What holding an orbit around a special body gives, once; then it moves to another body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialKind {
    /// Twice the shells, fired twice as often, until the ship is lost.
    Ammo,
    /// A fifth of the base's health back (see [`REPAIR_HP`]).
    Repair,
    /// The ship survives one shell.
    Shield,
}

impl SpecialKind {
    pub const ALL: [SpecialKind; 3] = [SpecialKind::Ammo, SpecialKind::Repair, SpecialKind::Shield];

    pub fn name(self) -> &'static str {
        match self {
            SpecialKind::Ammo => "ammo",
            SpecialKind::Repair => "repair",
            SpecialKind::Shield => "shield",
        }
    }
}

/// Seconds on a qualifying orbit around a special body to take what it gives.
pub const SPECIAL_HOLD_SECONDS: f64 = 4.0;

/// A body that gives something to whoever holds an orbit around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Special {
    pub kind: SpecialKind,
    pub slot: u32,
}

/// A body to orbit for points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Objective {
    pub slot: u32,
    /// Ticks before it moves on to another body (see `GameRules::target_ticks`).
    pub left: u32,
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
    pub mode: GameMode,
    pub preset: String,
    pub round: u32,
    /// Tick at which the current round ends (`None`: endless).
    pub round_end_tick: Option<Tick>,
    /// Set between rounds: the tick at which the next world starts.
    pub next_round_tick: Option<Tick>,
    /// The bodies to orbit for points.
    pub objectives: Vec<Objective>,
    pub bases: Vec<Base>,
    pub specials: Vec<Special>,
    pub rules: GameRules,
    pub massive: MassiveSnapshot,
    pub names: Vec<(u32, String)>,
    pub players: Vec<PlayerInfo>,
    pub shells: Vec<ShellInfo>,
}

/// Reliable, ordered, tick-stamped facts. A `tick` is the first tick at which the fact holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    PlayerJoined { id: PlayerId, name: String, color: [u8; 3] },
    PlayerColor { id: PlayerId, color: [u8; 3] },
    PlayerLeft { id: PlayerId },
    /// From `tick` on the player watches (and has no ship), or plays again (and gets one).
    Gamemode { tick: Tick, player: PlayerId, spectating: bool },
    ShipSpawn { tick: Tick, player: PlayerId, state: ShipState },
    Input { player: PlayerId, tick: Tick, input: ShipInput },
    ShellSpawn { tick: Tick, id: ShellId, owner: PlayerId, p: Particle },
    ShellGone { tick: Tick, id: ShellId, exploded: bool },
    ShipDied { tick: Tick, player: PlayerId, killer: Option<PlayerId>, body: Option<u32>, respawn_tick: Tick },
    /// A player's totals were set by hand.
    Score { player: PlayerId, kills: u32, deaths: u32, captures: u32 },
    /// The round now ends at this tick (`None`: never).
    RoundClock { round_end_tick: Option<Tick> },
    /// The bodies to orbit for points are now these.
    Objectives { tick: Tick, objectives: Vec<Objective> },
    /// `player` held an orbit around `target` long enough.
    Captured { tick: Tick, player: PlayerId, target: u32 },
    /// The special bodies are now these.
    Specials { tick: Tick, specials: Vec<Special> },
    /// `player` held an orbit around the special on `slot` long enough and got what it gives.
    Gift { tick: Tick, player: PlayerId, kind: SpecialKind, slot: u32 },
    /// The shield of `player`'s ship took a shell and is gone.
    ShieldSpent { tick: Tick, player: PlayerId },
    /// `by` brought the base of `owner` down to 0 hp.
    BaseDestroyed { tick: Tick, owner: PlayerId, by: Option<PlayerId> },
    /// Scores are final; a new world starts at `next_round_tick`. `winner`: the last player in play.
    RoundOver { tick: Tick, next_round_tick: Tick, winner: Option<PlayerId> },
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
    CmdAck { seq: u32, tick: Tick },
    Event(Event),
    /// Reliable: a line for the chat log.
    Chat(ChatLine),
    /// Reliable: `player` pointed at this spot on the map.
    Mark { player: PlayerId, x: f64, y: f64 },
    /// Reliable: whether the receiver may use operator commands.
    Operator(bool),
    /// Unreliable, a few times a second: how long each objective has left, and who has held
    /// a qualifying orbit around which body for how many ticks.
    Progress { objectives: Vec<Objective>, holds: Vec<(PlayerId, u32, u32)> },
    /// Unreliable, a few times a second while bases change: their health, who is besieging
    /// whose base (player, base owner), and who has held an orbit around which special body
    /// for how many ticks (player, slot, ticks).
    Bases { bases: Vec<Base>, sieges: Vec<(PlayerId, PlayerId)>, holds: Vec<(PlayerId, u32, u32)> },
    /// Unreliable safety net: authoritative ship state at the start of `tick`.
    ShipCheck { tick: Tick, player: PlayerId, state: ShipState },
}

/// A ship colour that stands out from the dark sky: one that is too dark is lightened.
pub fn visible_color(c: [u8; 3]) -> [u8; 3] {
    let luma = (30 * c[0] as u32 + 59 * c[1] as u32 + 11 * c[2] as u32) / 100;
    let lift = 90u32.saturating_sub(luma) as u8;
    c.map(|v| v.saturating_add(lift))
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
        !matches!(self, ClientMsg::Cmds(_) | ClientMsg::Ping { .. })
    }
}

impl ServerMsg {
    pub fn reliable(&self) -> bool {
        !matches!(self, ServerMsg::Pong { .. } | ServerMsg::ShipCheck { .. } | ServerMsg::Progress { .. } | ServerMsg::Bases { .. })
    }
}

/// Transport channel layout, identical on both ends.
#[cfg(not(target_arch = "wasm32"))]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ChatKind {
    /// Said to everyone by `from`.
    Say,
    /// Private message; `from` is the other party, `outgoing` tells which way it went.
    Private { outgoing: bool },
    /// From the server: command output, notices.
    System,
    /// A command went wrong (only the sender sees it).
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatLine {
    pub kind: ChatKind,
    pub from: Option<String>,
    pub text: String,
}
