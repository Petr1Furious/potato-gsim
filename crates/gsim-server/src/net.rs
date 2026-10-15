//! Real-time driver: the UDP transport, the one for browsers, and the fixed-rate tick loop.

use crate::authority::{Authority, TargetPlan};
use crate::scenario::{self, Params};
use gsim_core::GameRules;
use gsim_proto::*;
use renet::{RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};
use crate::state::ServerState;
use crate::web::{Event, WebServer};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub bind: SocketAddr,
    /// Address clients dial, if different from `bind` (NAT, containers).
    pub public_addr: Option<SocketAddr>,
    pub max_clients: usize,
    /// Where browsers connect (TCP), if anywhere, and the web client's files to hand them.
    pub web: Option<SocketAddr>,
    pub web_dir: Option<PathBuf>,
    pub preset: String,
    pub seed: u64,
    pub time_scale: f64,
    pub tick_hz: u32,
    /// Settings for the preset (see `scenario::PRESETS`); keys left out take their defaults.
    pub params: Params,
    pub quiet: bool,
    /// 0 = endless.
    pub round_seconds: f64,
    pub intermission_seconds: f64,
    /// One target for every so many players, and at least one (2 when neither this nor
    /// `targets` is given). Not together with `targets`.
    pub players_per_target: Option<u32>,
    /// A fixed number of targets instead, however many players there are.
    pub targets: Option<u32>,
    /// How long a target stays on one body before it moves on (0 = until it is captured).
    pub target_seconds: f64,
    /// Where name ownership, whitelist and bans are kept (`None`: in memory only).
    pub state_dir: Option<PathBuf>,
    /// Only let whitelisted names in.
    pub whitelist: bool,
    /// What rounds are about.
    pub mode: GameMode,
    /// Treat every player as an operator (solo play).
    pub op_all: bool,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            public_addr: None,
            max_clients: 32,
            web: None,
            web_dir: None,
            preset: "random".into(),
            seed: 1,
            time_scale: 86400.0,
            tick_hz: 60,
            params: Params::new(),
            quiet: false,
            round_seconds: 600.0,
            intermission_seconds: 10.0,
            players_per_target: None,
            targets: None,
            target_seconds: 90.0,
            state_dir: None,
            whitelist: false,
            mode: GameMode::Targets,
            op_all: false,
        }
    }
}

/// Catch up at most this many ticks per loop iteration, and forget older debt, so a stall
/// slows the game down instead of freezing the process.
const MAX_STEPS_PER_LOOP: u32 = 8;
const MAX_DEBT_SECONDS: f64 = 0.25;

pub fn build_authority(opts: &ServerOptions) -> Result<Authority, String> {
    if !(opts.time_scale.is_finite() && opts.time_scale > 0.0) {
        return Err("time scale must be positive".into());
    }
    let sc = scenario::build(&opts.preset, opts.seed, &opts.params)?;
    let mut rules = GameRules::new(opts.time_scale, opts.tick_hz.clamp(10, 240));
    rules.escape_radius = sc.escape_radius();
    let hz = rules.tick_hz as f64;
    let ticks = |s: f64| if s.is_finite() && s > 0.0 { (s * hz).round() as u64 } else { 0 };
    rules.target_ticks = ticks(opts.target_seconds) as u32;
    let target_plan = match (opts.players_per_target, opts.targets) {
        (Some(_), Some(_)) => return Err("the number of targets is set either per players or as a whole, not both".into()),
        (None, Some(count)) => TargetPlan::Fixed(count),
        (each, None) => TargetPlan::PerPlayers(each.unwrap_or(2).max(1)),
    };
    let mut authority = Authority::new(sc, rules, opts.seed);
    authority.target_plan = target_plan;
    authority.state = match &opts.state_dir {
        Some(dir) => ServerState::open(dir, opts.whitelist)?,
        None => ServerState::in_memory(),
    };
    authority.state.whitelist_enabled = opts.whitelist;
    authority.op_all = opts.op_all;
    authority.set_rounds(ticks(opts.round_seconds), ticks(opts.intermission_seconds).max(1), Some((opts.preset.clone(), opts.params.clone())));
    authority.enable_objective();
    authority.set_mode(opts.mode);
    Ok(authority)
}

/// Run until `stop` is set. Returns an error only if the socket cannot be set up.
pub fn run(opts: ServerOptions, stop: Arc<AtomicBool>) -> Result<(), String> {
    let started = Instant::now();
    let mut authority = build_authority(&opts)?;
    let socket = UdpSocket::bind(opts.bind).map_err(|e| format!("bind {}: {e}", opts.bind))?;
    let local = socket.local_addr().map_err(|e| e.to_string())?;
    let public = opts.public_addr.unwrap_or(local);
    let config = ServerConfig {
        current_time: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap(),
        max_clients: opts.max_clients.clamp(1, 64),
        protocol_id: netcode_protocol_id(),
        public_addresses: vec![public],
        authentication: ServerAuthentication::Unsecure,
    };
    let mut transport = NetcodeServerTransport::new(config, socket).map_err(|e| e.to_string())?;
    let mut server = RenetServer::new(connection_config());
    let mut web = match opts.web {
        Some(bind) => Some(WebServer::bind(bind, opts.web_dir.clone(), opts.max_clients.clamp(1, 64))?),
        None => None,
    };

    let period = 1.0 / authority.rules.tick_hz as f64;
    let say = |level: &str, text: &str| {
        if !opts.quiet {
            crate::log(level, text);
        }
    };
    say("INFO", &format!("Starting gsim server version {} (protocol {})", env!("CARGO_PKG_VERSION"), gsim_proto::PROTOCOL_VERSION));
    say("INFO", &format!("Game mode: {}, world: {}, {} Hz, time x{}", authority.mode().name(), opts.preset, authority.rules.tick_hz, opts.time_scale));
    say("INFO", &format!("Starting gsim server on {local}{}", opts.web.map_or(String::new(), |web| format!(" (browsers on tcp {web})"))));
    say("INFO", &format!("Done ({:.3}s)!", started.elapsed().as_secs_f64()));

    let mut last = Instant::now();
    let mut debt = 0.0f64;
    let mut report_at = Instant::now() + Duration::from_secs(10);
    let mut reload_at = Instant::now();
    // Connections being refused: closed a moment later so the reason reaches them first.
    let mut closing: Vec<(u64, Instant)> = Vec::new();
    // Simulated time given up because the steps could not keep up.
    let mut dropped = 0.0f64;

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let elapsed = now - last;
        last = now;
        server.update(elapsed);
        if let Err(e) = transport.update(elapsed, &mut server) {
            say("WARN", &format!("Network error: {e}"));
        }
        debt += elapsed.as_secs_f64();
        if debt > MAX_DEBT_SECONDS {
            dropped += debt - MAX_DEBT_SECONDS;
            debt = MAX_DEBT_SECONDS;
        }

        while let Some(event) = server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    authority.connected(client_id, transport.client_addr(client_id).map(|a| a.ip()));
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    if let Some(name) = authority.player_of(client_id).and_then(|id| authority.name(id)) {
                        say("INFO", &format!("{name} lost connection: {reason}"));
                    }
                    authority.disconnect(client_id);
                }
            }
        }
        let tick_frac = authority.tick() as f64 + (debt / period).min(0.999);
        for event in web.iter_mut().flat_map(|web| web.poll()) {
            match event {
                Event::Connected(conn, ip) => authority.connected(conn, Some(ip)),
                Event::Disconnected(conn) => authority.disconnect(conn),
                Event::Message(conn, bytes) => {
                    if let Some(msg) = decode::<ClientMsg>(&bytes) {
                        authority.handle(conn, msg, tick_frac);
                    }
                }
            }
        }
        for client in server.clients_id() {
            for channel in [CH_RELIABLE, CH_UNRELIABLE] {
                while let Some(bytes) = server.receive_message(client, channel) {
                    if let Some(msg) = decode::<ClientMsg>(&bytes) {
                        authority.handle(client, msg, tick_frac);
                    }
                }
            }
        }

        if authority.player_count() == 0 {
            // Nothing to simulate for: the world waits, and a fresh round starts on the next join.
            debt = 0.0;
        }
        let mut n = 0;
        while debt >= period && n < MAX_STEPS_PER_LOOP {
            authority.step();
            debt -= period;
            n += 1;
        }

        for out in authority.drain_out() {
            let channel = if out.msg.reliable() { CH_RELIABLE } else { CH_UNRELIABLE };
            let bytes = encode(&out.msg);
            for conn in out.to {
                if server.is_connected(conn) {
                    server.send_message(conn, channel, bytes.clone());
                } else if let Some(web) = &mut web {
                    web.send(conn, &bytes);
                }
            }
        }
        transport.send_packets(&mut server);
        if let Some(web) = &mut web {
            web.flush();
        }
        closing.extend(authority.drain_kicks().into_iter().map(|c| (c, now + Duration::from_millis(400))));
        closing.retain(|(conn, at)| {
            let due = Instant::now() >= *at;
            if due && server.is_connected(*conn) {
                server.disconnect(*conn);
            } else if let (true, Some(web)) = (due, &mut web) {
                web.close(*conn);
            }
            !due
        });
        if now >= reload_at {
            // Edits made by `gsim-server admin ...` or by hand take effect here.
            reload_at = now + Duration::from_secs(2);
            authority.reload_state();
        }

        for (level, text) in authority.drain_log() {
            say(level, &text);
        }
        // Falling behind, as Minecraft says it (at most every 15 s).
        if dropped > 0.5 && now >= report_at {
            report_at = now + Duration::from_secs(15);
            let ticks = (dropped / period).round();
            say("WARN", &format!("Can't keep up! Is the server overloaded? Running {:.0}ms or {ticks:.0} ticks behind", dropped * 1e3));
            dropped = 0.0;
        }

        // Sleep until the next tick is due, but keep servicing the socket.
        let wait = (period - debt).clamp(0.0, 0.002);
        std::thread::sleep(Duration::from_secs_f64(wait.max(0.0002)));
    }
    say("INFO", "Stopping server");
    transport.disconnect_all(&mut server);
    if let Some(web) = &mut web {
        web.close_all();
    }
    Ok(())
}
