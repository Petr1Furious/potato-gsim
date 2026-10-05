use clap::Parser;
use gsim_server::net::{run, ServerOptions};
use gsim_server::scenario::{RandomOpts, PRESETS};
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
    /// Body count for the random/disc/repulsor presets
    #[arg(long, env = "GSIM_RANDOM_COUNT", default_value_t = 1000)]
    random_count: usize,
    /// Disc radius (m) for the random/repulsor presets
    #[arg(long, env = "GSIM_RANDOM_SPREAD", default_value_t = 5.0e10)]
    random_spread: f64,
    #[arg(long, default_value_t = 1.0e21)]
    random_mass_min: f64,
    #[arg(long, default_value_t = 5.0e25)]
    random_mass_max: f64,
    /// Initial rotation as a fraction of circular speed (0 = static cloud)
    #[arg(long, default_value_t = 0.7)]
    random_rotation: f64,
    #[arg(long)]
    list_presets: bool,
    /// Central star mass for the random/repulsor presets (kg, 0 = none)
    #[arg(long, env = "GSIM_RANDOM_STAR_MASS", default_value_t = 0.0)]
    random_star_mass: f64,
    /// Round length in seconds (0 = endless, the world is never reset)
    #[arg(long, env = "GSIM_ROUND_SECONDS", default_value_t = 600.0)]
    round_seconds: f64,
    /// Pause between rounds in seconds
    #[arg(long, env = "GSIM_INTERMISSION_SECONDS", default_value_t = 10.0)]
    intermission_seconds: f64,
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
        for (name, about) in PRESETS {
            println!("{name:10} {about}");
        }
        return;
    }
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
        random: RandomOpts {
            count: args.random_count.clamp(1, 5000),
            spread: args.random_spread,
            mass_min: args.random_mass_min,
            mass_max: args.random_mass_max,
            rotation: args.random_rotation,
            star_mass: args.random_star_mass,
        },
        quiet: false,
        round_seconds: args.round_seconds,
        intermission_seconds: args.intermission_seconds,
        state_dir: Some(args.state_dir.clone()),
        whitelist: args.whitelist,
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
        Admin::Forget { name } => done(state.forget(name), format!("{name} is free to claim again"), format!("{name} is not registered")),
    }
}
