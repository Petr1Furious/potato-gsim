use clap::Parser;
use gsim_server::net::{run, ServerOptions};
use gsim_server::scenario::{self, format_value, Params, PRESETS};
use clap::Subcommand;
use gsim_server::state::ServerState;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Authoritative server for potato-gsim.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Address to bind
    #[arg(long, env = "GSIM_BIND", default_value = "0.0.0.0")]
    bind: IpAddr,
    /// UDP port
    #[arg(short, long, env = "GSIM_PORT", default_value_t = gsim_proto::DEFAULT_PORT)]
    port: u16,
    /// Address clients connect to, if it differs from the bind address (e.g. behind NAT)
    #[arg(long, env = "GSIM_PUBLIC_ADDR")]
    public_addr: Option<SocketAddr>,
    #[arg(long, env = "GSIM_MAX_CLIENTS", default_value_t = 32)]
    max_clients: usize,
    /// World preset (see --list-presets)
    #[arg(long, env = "GSIM_PRESET", default_value = "random")]
    preset: String,
    /// World seed; 0 picks one from the clock
    #[arg(long, env = "GSIM_SEED", default_value_t = 0)]
    seed: u64,
    /// Simulated seconds per real second
    #[arg(long, env = "GSIM_TIME_SCALE", default_value_t = 86400.0)]
    time_scale: f64,
    /// Simulation ticks per real second
    #[arg(long, env = "GSIM_TICK_HZ", default_value_t = 60)]
    tick_hz: u32,
    /// Tune the preset: `--set count=300 --set spread=80Gm` (repeatable; the environment
    /// variable takes a comma- or space-separated list). See --list-presets for the keys.
    #[arg(long = "set", env = "GSIM_SET", value_name = "KEY=VALUE")]
    set: Vec<String>,
    /// Print every preset with its parameters, defaults and ranges
    #[arg(long)]
    list_presets: bool,
    /// Round length in seconds (0 = endless, the world is never reset)
    #[arg(long, env = "GSIM_ROUND_SECONDS", default_value_t = 600.0)]
    round_seconds: f64,
    /// Pause between rounds in seconds
    #[arg(long, env = "GSIM_INTERMISSION_SECONDS", default_value_t = 10.0)]
    intermission_seconds: f64,
    /// One target for every so many players, and at least one [default: 2]
    #[arg(long, env = "GSIM_PLAYERS_PER_TARGET", conflicts_with = "targets", value_parser = clap::value_parser!(u32).range(1..))]
    players_per_target: Option<u32>,
    /// A fixed number of targets instead, however many players there are
    #[arg(long, env = "GSIM_TARGETS", value_parser = clap::value_parser!(u32).range(1..=64))]
    targets: Option<u32>,
    /// Seconds a target stays on one body before it moves on (0 = until it is captured)
    #[arg(long, env = "GSIM_TARGET_SECONDS", default_value_t = 90.0)]
    target_seconds: f64,
    /// Directory for player names, whitelist and ban lists
    #[arg(long, env = "GSIM_STATE_DIR", default_value = "data", global = true)]
    state_dir: PathBuf,
    /// Only let players on the whitelist join
    #[arg(long, env = "GSIM_WHITELIST")]
    whitelist: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

fn main() {
    let args = Args::parse();
    if let Some(Command::Admin { action }) = &args.command {
        std::process::exit(admin(&args.state_dir, action));
    }
    if args.list_presets {
        for p in PRESETS {
            println!("{:10} {}", p.name, p.about);
            for s in p.params {
                println!("    {:11} {:10} {}  [{}]", s.key, format_value(s.default), s.help, s.range());
            }
        }
        return;
    }
    let params = match settings(&args.preset, &args.set) {
        Ok(params) => params,
        Err(e) => {
            eprintln!("error: {e} (see --list-presets)");
            std::process::exit(2);
        }
    };
    let seed = if args.seed != 0 {
        args.seed
    } else {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1)
    };
    let opts = ServerOptions {
        bind: SocketAddr::new(args.bind, args.port),
        public_addr: args.public_addr,
        max_clients: args.max_clients,
        preset: args.preset,
        seed,
        time_scale: args.time_scale,
        tick_hz: args.tick_hz,
        params,
        quiet: false,
        round_seconds: args.round_seconds,
        intermission_seconds: args.intermission_seconds,
        players_per_target: args.players_per_target,
        targets: args.targets,
        target_seconds: args.target_seconds,
        state_dir: Some(args.state_dir.clone()),
        whitelist: args.whitelist,
        op_all: false,
    };
    if !gsim_core::selftest::passes() {
        eprintln!("FATAL: simulation self-test failed on this machine; refusing to host.");
        std::process::exit(2);
    }
    if let Err(e) = run(opts, Arc::new(AtomicBool::new(false))) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// The `--set` words (each may hold several settings) as checked parameters of `preset`.
fn settings(preset: &str, words: &[String]) -> Result<Params, String> {
    let mut params = Params::new();
    for word in words.iter().flat_map(|w| w.split([',', ' ']).filter(|s| !s.is_empty())) {
        let (key, value) = scenario::parse_setting(word)?;
        params.insert(key, value);
    }
    scenario::validate(preset, &params)?;
    Ok(params)
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Manage bans, the whitelist and registered names. A running server picks changes up
    /// within a couple of seconds and removes players who are no longer allowed.
    Admin {
        #[command(subcommand)]
        action: Admin,
    },
}

#[derive(Subcommand, Debug)]
enum Admin {
    /// List registered players (name, key, last address)
    Players,
    /// Ban a player by name
    Ban { name: String, reason: Vec<String> },
    Unban { name: String },
    /// Ban an address, or the last address of a named player
    BanIp { target: String, reason: Vec<String> },
    UnbanIp { ip: IpAddr },
    /// Show both ban lists
    Bans,
    /// Add a name to the whitelist
    Allow { name: String },
    /// Remove a name from the whitelist
    Disallow { name: String },
    /// Show the whitelist
    Whitelist,
    /// Let a player use operator commands in chat
    Op { name: String },
    Deop { name: String },
    /// Show the operators
    Ops,
    /// Release a registered name so anyone can claim it
    Forget { name: String },
}

fn admin(dir: &Path, action: &Admin) -> i32 {
    let mut state = match ServerState::open(dir, false) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let done = |ok: bool, yes: String, no: String| {
        println!("{}", if ok { yes } else { no });
        if ok { 0 } else { 1 }
    };
    match action {
        Admin::Players => {
            for p in state.players() {
                let key = gsim_proto::identity::fingerprint(&p.key);
                println!("{:24} key {key}  last address {}", p.name, p.last_ip.map_or("-".to_string(), |ip| ip.to_string()));
            }
            0
        }
        Admin::Ban { name, reason } => {
            state.ban(name, &reason.join(" "));
            done(true, format!("banned {name}"), String::new())
        }
        Admin::Unban { name } => done(state.unban(name), format!("unbanned {name}"), format!("{name} was not banned")),
        Admin::BanIp { target, reason } => {
            let ip = target.parse::<IpAddr>().ok().or_else(|| state.record(target).and_then(|r| r.last_ip));
            match ip {
                Some(ip) => {
                    state.ban_ip(ip, &reason.join(" "));
                    done(true, format!("banned address {ip}"), String::new())
                }
                None => done(false, String::new(), format!("{target} is neither an address nor a player with a known address")),
            }
        }
        Admin::UnbanIp { ip } => done(state.unban_ip(*ip), format!("unbanned {ip}"), format!("{ip} was not banned")),
        Admin::Bans => {
            for (name, reason) in state.banned_players() {
                println!("player  {name:24} {reason}");
            }
            for (ip, reason) in state.banned_ips() {
                println!("address {:24} {reason}", ip.to_string());
            }
            0
        }
        Admin::Allow { name } => {
            state.whitelist_add(name);
            done(true, format!("{name} added to the whitelist"), String::new())
        }
        Admin::Disallow { name } => done(state.whitelist_remove(name), format!("{name} removed from the whitelist"), format!("{name} was not on the whitelist")),
        Admin::Whitelist => {
            for name in state.whitelist() {
                println!("{name}");
            }
            0
        }
        Admin::Op { name } => {
            state.op(name);
            done(true, format!("{name} is now an operator"), String::new())
        }
        Admin::Deop { name } => done(state.deop(name), format!("{name} is no longer an operator"), format!("{name} was not an operator")),
        Admin::Ops => {
            for name in state.ops() {
                println!("{name}");
            }
            0
        }
        Admin::Forget { name } => done(state.forget(name), format!("{name} is free to claim again"), format!("{name} is not registered")),
    }
}
