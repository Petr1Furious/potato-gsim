//! Wire protocol. The server never streams positions: it sends a snapshot once, then only
//! tick-stamped inputs and events. Clients reproduce everything else themselves.

use gsim_core::{GameRules, Magazine, MassiveSnapshot, Particle, ShipInput, ShipState, Tick};
use serde::{Deserialize, Serialize};

/// Bump on any wire or simulation change.
pub const PROTOCOL_VERSION: u32 = 23;
pub const DEFAULT_PORT: u16 = 27777;
pub const MAX_NAME_CHARS: usize = 24;
/// Commands are re-sent until acknowledged; this bounds one packet.
pub const MAX_CMDS_PER_PACKET: usize = 48;
pub const MAX_CHAT_CHARS: usize = 200;

pub type PlayerId = u32;
pub type ShellId = u32;
pub type PieceId = u32;
/// A side in a round of bases: one player, or several sharing a base (0: none).
pub type TeamId = u32;

pub mod command;
pub mod identity;
pub mod shop;
pub use identity::Identity;
pub use shop::{Items, PieceKind, Purse, Supply, Upgrade, Upgrades, Ware};

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
    /// Reliable: buy this (at home, from what is banked).
    Buy(Ware),
    /// Reliable: use one of the supplies the ship carries (a gravity well is fired with this
    /// heading and speed, as a shell is).
    Use { supply: Supply, angle: u16, speed: f32 },
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
    /// Their side in a round of bases.
    pub team: TeamId,
    /// The ship survives one shell.
    pub shield: bool,
    /// What the ship carries, what is banked at home, and what has been bought.
    pub cargo: Purse,
    pub bank: Purse,
    pub upgrades: Upgrades,
    pub items: Items,
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

/// What makes a round of bases unlike the others, rolled when it starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roll {
    /// Scrap floating about, in percent of the usual.
    pub scrap: u32,
    /// Mines beyond the usual.
    pub mines: u32,
    /// Players on a team (1: every one for themselves).
    pub teams: u32,
    pub twist: Twist,
}

/// One rule bent for a round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Twist {
    #[default]
    Plain,
    /// Twice the delta-v, coming back twice as fast.
    DeepTanks,
    /// Shells fired and coming back twice as fast.
    QuickGuns,
    /// Shells half as fast again, and lasting twice as long.
    LongShots,
    /// Two and a half times the scrap.
    ScrapRush,
    /// Crystals at the centre every 30 s, and more of them.
    CrystalFever,
    /// Ramming a base does two and a half times the damage.
    HeavyRams,
}

impl Twist {
    pub const ALL: [Twist; 7] = [Twist::Plain, Twist::DeepTanks, Twist::QuickGuns, Twist::LongShots, Twist::ScrapRush, Twist::CrystalFever, Twist::HeavyRams];

    pub fn name(self) -> &'static str {
        match self {
            Twist::Plain => "no twist",
            Twist::DeepTanks => "deep tanks",
            Twist::QuickGuns => "quick guns",
            Twist::LongShots => "long shots",
            Twist::ScrapRush => "scrap rush",
            Twist::CrystalFever => "crystal fever",
            Twist::HeavyRams => "heavy rams",
        }
    }

    /// How commands name it.
    pub fn key(self) -> &'static str {
        match self {
            Twist::Plain => "plain",
            Twist::DeepTanks => "deep-tanks",
            Twist::QuickGuns => "quick-guns",
            Twist::LongShots => "long-shots",
            Twist::ScrapRush => "scrap-rush",
            Twist::CrystalFever => "crystal-fever",
            Twist::HeavyRams => "heavy-rams",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Twist::Plain => "the usual rules",
            Twist::DeepTanks => "twice the delta-v",
            Twist::QuickGuns => "shells fire and come back twice as fast",
            Twist::LongShots => "faster shells that fly twice as long",
            Twist::ScrapRush => "scrap everywhere",
            Twist::CrystalFever => "crystals at the centre every 30 s",
            Twist::HeavyRams => "ramming a base does 2.5x the damage",
        }
    }
}

/// Seconds into a round of bases when sudden death starts, and what every base then loses a
/// second.
pub const SUDDEN_DEATH_SECONDS: f64 = 720.0;
pub const SUDDEN_DEATH_PER_SECOND: u32 = BASE_HP / 100;

/// A base's health when it is new.
pub const BASE_HP: u32 = 6000;

/// What a repair gives back to a standing base.
pub const REPAIR_HP: u32 = BASE_HP / 5;

/// How far around one's own base (in base radii) shells cost nothing: the home zone. Ships
/// spawn inside it, and it is as far as the base pulls ships in full.
pub const HOME_RADII: f64 = 100.0;

/// How big a base is (m): an Earth, small enough for a ship to circle close in.
pub const BASE_RADIUS: f64 = 6.4e6;

/// What teams are called (and coloured), from team 1 on.
pub const TEAM_NAMES: [&str; 8] = ["red", "blue", "green", "gold", "violet", "cyan", "orange", "white"];

pub fn team_name(team: TeamId) -> &'static str {
    TEAM_NAMES[(team.max(1) as usize - 1) % TEAM_NAMES.len()]
}

/// A team's base: a body that does not move. It is gone (a hulk) at 0 hp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Base {
    pub team: TeamId,
    pub slot: u32,
    pub hp: u32,
}

/// Seconds on a qualifying orbit around a mine for each crystal it gives.
pub const MINE_SECONDS: f64 = 16.0;
/// Crystals in a mine when it opens; once they are gone it moves to another body.
pub const MINE_STOCK: u32 = 3;

/// A body that gives crystals to whoever holds an orbit around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mine {
    pub slot: u32,
    /// Crystals still in it.
    pub left: u32,
}

/// A body to orbit for points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Objective {
    pub slot: u32,
    /// Ticks before it moves on to another body (see `GameRules::target_ticks`).
    pub left: u32,
}

/// Everything a player has, besides their ship: what it carries, what is banked at home,
/// what has been bought, and its guns and shield.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Loadout {
    pub cargo: Purse,
    pub bank: Purse,
    pub upgrades: Upgrades,
    pub items: Items,
    pub magazine: Magazine,
    pub shield: bool,
    /// Twice the shells, fired twice as fast (from ammo, until the ship is lost).
    pub quick: bool,
}

/// A piece of scrap or a crystal, floating free.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PieceInfo {
    pub id: PieceId,
    pub kind: PieceKind,
    pub p: Particle,
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
    /// From this tick on every base wears down (bases mode).
    pub sudden_death: Option<Tick>,
    /// How this round differs from the others.
    pub roll: Roll,
    /// The bodies to orbit for points.
    pub objectives: Vec<Objective>,
    pub bases: Vec<Base>,
    pub mines: Vec<Mine>,
    /// When crystals next turn up near the centre.
    pub crystals_due: Option<Tick>,
    /// Gravity wells on their way or pulling: (tick fired, state at `massive.tick`).
    pub wells: Vec<(Tick, Particle)>,
    pub rules: GameRules,
    pub massive: MassiveSnapshot,
    pub names: Vec<(u32, String)>,
    pub players: Vec<PlayerInfo>,
    pub shells: Vec<ShellInfo>,
    pub pieces: Vec<PieceInfo>,
}

/// Reliable, ordered, tick-stamped facts. A `tick` is the first tick at which the fact holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    PlayerJoined { id: PlayerId, name: String, color: [u8; 3], team: TeamId },
    PlayerColor { id: PlayerId, color: [u8; 3] },
    PlayerLeft { id: PlayerId },
    /// From `tick` on the player watches (and has no ship), or plays again (and gets one).
    Gamemode { tick: Tick, player: PlayerId, spectating: bool },
    ShipSpawn { tick: Tick, player: PlayerId, state: ShipState },
    Input { player: PlayerId, tick: Tick, input: ShipInput },
    /// `free`: fired from within the owner's home zone, so it took nothing from the magazine.
    ShellSpawn { tick: Tick, id: ShellId, owner: PlayerId, p: Particle, free: bool },
    ShellGone { tick: Tick, id: ShellId, exploded: bool },
    ShipDied { tick: Tick, player: PlayerId, killer: Option<PlayerId>, body: Option<u32>, respawn_tick: Tick },
    /// A player's totals were set by hand.
    Score { player: PlayerId, kills: u32, deaths: u32, captures: u32 },
    /// The round now ends at this tick (`None`: never), and sudden death starts at this one.
    RoundClock { round_end_tick: Option<Tick>, sudden_death: Option<Tick> },
    /// The bodies to orbit for points are now these.
    Objectives { tick: Tick, objectives: Vec<Objective> },
    /// `player` held an orbit around `target` long enough.
    Captured { tick: Tick, player: PlayerId, target: u32 },
    /// The mines are now these.
    Mines { tick: Tick, mines: Vec<Mine> },
    /// `player` held an orbit around the mine on `slot` long enough for a crystal.
    Mined { tick: Tick, player: PlayerId, slot: u32 },
    /// Crystals turn up near the centre at `due`.
    CrystalsDue { due: Option<Tick> },
    /// `player` fired a gravity well at `tick`, from `p`. It flies as a shell does and pulls
    /// ships in for `GameRules::well_ticks` from `GameRules::well_arm_ticks` on.
    Well { tick: Tick, player: PlayerId, p: Particle },
    /// `player` jumped home, leaving their cargo behind.
    Warped { tick: Tick, player: PlayerId },
    /// The shield of `player`'s ship took a shell and is gone.
    ShieldSpent { tick: Tick, player: PlayerId },
    /// `by` (nobody: sudden death) brought the base of `team` down to 0 hp.
    BaseDestroyed { tick: Tick, team: TeamId, by: Option<PlayerId> },
    /// Scores are final; a new world starts at `next_round_tick`. `winner`: the last team in play.
    RoundOver { tick: Tick, next_round_tick: Tick, winner: Option<TeamId> },
    /// These pieces are floating free from `tick` on.
    Pieces { tick: Tick, pieces: Vec<PieceInfo> },
    /// The piece is gone from `tick` on: taken by `by`, or lost (crashed, or far out).
    PieceGone { tick: Tick, id: PieceId, by: Option<PlayerId> },
    /// From `tick` on the player carries `cargo`, has `bank` at home and these upgrades and
    /// shells (the ship's engine and tank follow from them, see [`shop::kit`]).
    Loadout { tick: Tick, player: PlayerId, loadout: Loadout },
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
    /// whose base (player, team), and who has held an orbit around which mine for how many
    /// ticks (player, slot, ticks).
    /// `hits`: (team, by whom) for every base hurt since the last of these.
    Bases { bases: Vec<Base>, sieges: Vec<(PlayerId, TeamId)>, holds: Vec<(PlayerId, u32, u32)>, hits: Vec<(TeamId, PlayerId)> },
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
