//! Real-time driver: UDP transport plus the fixed-rate tick loop.

use crate::authority::Authority;
use crate::scenario::{self, RandomOpts};
use gsim_core::GameRules;
use gsim_proto::*;
use renet::{RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub bind: SocketAddr,
    /// Address clients dial, if different from `bind` (NAT, containers).
    pub public_addr: Option<SocketAddr>,
    pub max_clients: usize,
    pub preset: String,
    pub seed: u64,
    pub time_scale: f64,
    pub tick_hz: u32,
    pub random: RandomOpts,
    pub quiet: bool,
    /// 0 = endless.
    pub round_seconds: f64,
    pub intermission_seconds: f64,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            public_addr: None,
            max_clients: 32,
            preset: "random".into(),
            seed: 1,
            time_scale: 86400.0,
            tick_hz: 60,
            random: RandomOpts::default(),
            quiet: false,
            round_seconds: 600.0,
            intermission_seconds: 10.0,
        }
    }
}

/// Catch up at most this many ticks per loop iteration, and forget older debt, so a stall
/// slows the game down instead of freezing the process.
const MAX_STEPS_PER_LOOP: u32 = 8;
const MAX_DEBT_SECONDS: f64 = 0.25;
/// Live clients send something several times a second.
const STALE_AFTER: Duration = Duration::from_millis(1500);

pub fn build_authority(opts: &ServerOptions) -> Result<Authority, String> {
    if !(opts.time_scale.is_finite() && opts.time_scale > 0.0) {
        return Err("time scale must be positive".into());
    }
    let sc = scenario::build(&opts.preset, opts.seed, &opts.random)?;
    let mut rules = GameRules::new(opts.time_scale, opts.tick_hz.clamp(10, 240));
    rules.escape_radius = sc.escape_radius();
    let hz = rules.tick_hz as f64;
    let mut authority = Authority::new(sc, rules, opts.seed);
    let ticks = |s: f64| if s.is_finite() && s > 0.0 { (s * hz).round() as u64 } else { 0 };
    authority.set_rounds(ticks(opts.round_seconds), ticks(opts.intermission_seconds).max(1), Some((opts.preset.clone(), opts.random.clone())));
    authority.enable_objective();
    Ok(authority)
}

/// Run until `stop` is set. Returns an error only if the socket cannot be set up.
pub fn run(opts: ServerOptions, stop: Arc<AtomicBool>) -> Result<(), String> {
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

    let period = 1.0 / authority.rules.tick_hz as f64;
    if !opts.quiet {
        eprintln!(
            "gsim-server listening on {local} | preset={} seed={} bodies={} | {} Hz, {:.0} sim-s per tick, time x{}",
            opts.preset,
            opts.seed,
            authority.massive.len(),
            authority.rules.tick_hz,
            authority.rules.dt,
            opts.time_scale
        );
    }

    let mut last = Instant::now();
    let mut debt = 0.0f64;
    let mut report_at = Instant::now() + Duration::from_secs(10);
    let (mut step_sum, mut step_max, mut steps, mut dropped) = (0.0f64, 0.0f64, 0u64, 0.0f64);

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let elapsed = now - last;
        last = now;
        server.update(elapsed);
        if let Err(e) = transport.update(elapsed, &mut server) {
            eprintln!("[net] transport error: {e}");
        }
        debt += elapsed.as_secs_f64();
        if debt > MAX_DEBT_SECONDS {
            dropped += debt - MAX_DEBT_SECONDS;
            debt = MAX_DEBT_SECONDS;
        }

        while let Some(event) = server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    if !opts.quiet {
                        eprintln!("[connect] conn {client_id}");
                    }
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    if !opts.quiet {
                        eprintln!("[disconnect] conn {client_id}: {reason}");
                    }
                    authority.disconnect(client_id);
                }
            }
        }
        let tick_frac = authority.tick() as f64 + (debt / period).min(0.999);
        for client in server.clients_id() {
            for channel in [CH_RELIABLE, CH_UNRELIABLE] {
                while let Some(bytes) = server.receive_message(client, channel) {
                    if let Some(msg) = decode::<ClientMsg>(&bytes) {
                        if let ClientMsg::Hello { name, .. } = &msg {
                            if !opts.quiet {
                                eprintln!("[hello] conn {client} name={name:?}");
                            }
                            // A client that vanished without saying goodbye (killed, crashed, lost
                            // network) still holds its name until the transport times out. A live
                            // client is never this quiet, so let the newcomer take the name over.
                            if let Some(old) = authority.holder_of(name).filter(|old| *old != client) {
                                let silent = transport.time_since_last_received_packet(old).unwrap_or(Duration::MAX);
                                if silent >= STALE_AFTER {
                                    if !opts.quiet {
                                        eprintln!("[takeover] conn {old} silent for {:.1} s, releasing {name:?}", silent.as_secs_f64());
                                    }
                                    server.disconnect(old);
                                    authority.disconnect(old);
                                }
                            }
                        }
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
            let t0 = Instant::now();
            authority.step();
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            step_sum += ms;
            step_max = step_max.max(ms);
            steps += 1;
            debt -= period;
            n += 1;
        }

        for out in authority.drain_out() {
            let channel = if out.msg.reliable() { CH_RELIABLE } else { CH_UNRELIABLE };
            let bytes = encode(&out.msg);
            for conn in out.to {
                if server.is_connected(conn) {
                    server.send_message(conn, channel, bytes.clone());
                }
            }
        }
        transport.send_packets(&mut server);

        if !opts.quiet && now >= report_at {
            report_at = now + Duration::from_secs(10);
            let s = authority.stats;
            eprintln!(
                "[status] tick={} players={} bodies={} step avg={:.2}ms max={:.2}ms | cmds={} late={} resyncs={}{}",
                authority.tick(),
                authority.player_count(),
                authority.massive.alive_count(),
                if steps > 0 { step_sum / steps as f64 } else { 0.0 },
                step_max,
                s.cmds,
                s.late_cmds,
                s.resyncs,
                if dropped > 0.05 { format!(" | STRESS: dropped {dropped:.2}s of sim time") } else { String::new() }
            );
            (step_sum, step_max, steps, dropped) = (0.0, 0.0, 0, 0.0);
        }

        // Sleep until the next tick is due, but keep servicing the socket.
        let wait = (period - debt).clamp(0.0, 0.002);
        std::thread::sleep(Duration::from_secs_f64(wait.max(0.0002)));
    }
    transport.disconnect_all(&mut server);
    Ok(())
}
