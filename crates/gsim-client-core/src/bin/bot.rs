//! Headless player. Flies around, shoots, and reports whether its world stayed in sync
//! with the server. Exit code 0 means no hash mismatch occurred.

use clap::Parser;
use gsim_client_core::net::{resolve, NetClient};
use gsim_client_core::{Controls, SessionConfig};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
struct Args {
    /// Server address (host or host:port)
    #[arg(default_value = "127.0.0.1")]
    server: String,
    #[arg(long, default_value = "bot")]
    name: String,
    /// Seconds to play before exiting
    #[arg(long, default_value_t = 30.0)]
    seconds: f64,
    /// Just sit there (no thrust, no shooting)
    #[arg(long)]
    passive: bool,
}

fn main() {
    let args = Args::parse();
    if !gsim_core::selftest::passes() {
        eprintln!("self-test FAILED: {:#018x} != {:#018x}", gsim_core::selftest::compute(), gsim_core::selftest::GOLDEN);
        std::process::exit(2);
    }
    let addr = resolve(&args.server).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let mut net = NetClient::connect(addr, SessionConfig::headless(&args.name)).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let start = Instant::now();
    let mut report = Instant::now();
    let mut seed = args.name.bytes().fold(0x1234_5678u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let mut controls = Controls::default();
    let mut next_change = 0.0;
    let mut joined_at = None;
    while start.elapsed().as_secs_f64() < args.seconds {
        let now = net.now();
        if !args.passive && now >= next_change {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            controls.angle = (seed >> 8) as u16;
            controls.thrust = if seed & 3 == 0 { 0 } else { 40 + (seed >> 24) as u8 % 61 };
            next_change = now + 0.15 + (seed >> 16 & 0xff) as f64 / 400.0;
        }
        controls.fire = (!args.passive && (now * 2.0) as u64 % 3 == 0).then_some((controls.angle.wrapping_add(8000), 6000.0));
        net.update(controls);
        if let Some(r) = &net.session.rejected {
            eprintln!("rejected: {r}");
            std::process::exit(1);
        }
        if let Some(r) = net.disconnect_reason() {
            eprintln!("disconnected: {r}");
            std::process::exit(1);
        }
        if joined_at.is_none() && net.session.joined() {
            joined_at = Some(start.elapsed());
            let w = net.session.world.as_ref().unwrap();
            println!("[{}] joined preset={} bodies={} after {:.0?}", args.name, w.preset, w.eph.end_tick().and_then(|e| w.eph.get(e - 1)).map_or(0, |r| r.props.alive.iter().filter(|a| **a).count()), start.elapsed());
        }
        if report.elapsed() > Duration::from_secs(5) {
            report = Instant::now();
            print_status(&args.name, &net);
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    print_status(&args.name, &net);
    net.disconnect();
    let s = net.session.stats;
    if !net.session.joined() || s.hash_checks == 0 || s.hash_mismatches > 0 {
        eprintln!("[{}] FAILED: joined={} checks={} mismatches={}", args.name, net.session.joined(), s.hash_checks, s.hash_mismatches);
        std::process::exit(3);
    }
    println!("[{}] OK", args.name);
}

fn print_status(name: &str, net: &NetClient) {
    let s = net.session.stats;
    let (up, down) = net.bytes_per_sec();
    let alive = net.session.world.as_ref().is_some_and(|w| w.my_ship().is_some());
    let score = net.session.world.as_ref().and_then(|w| w.me()).map_or((0, 0), |m| (m.kills, m.deaths));
    println!(
        "[{name}] rtt={:.1}ms lead={}t hash {}/{} bad | checks {} corrected {} | cmds {} retimed {} | remote-late {} rollbacks {} | resyncs {} | ship={} k/d={}/{} | {:.1}/{:.1} kB/s",
        net.session.rtt().unwrap_or(0.0) * 1e3,
        net.session.input_lead_ticks(),
        s.hash_checks,
        s.hash_mismatches,
        s.ship_checks,
        s.ship_corrections,
        s.cmds_sent,
        s.retimed_cmds,
        s.late_remote_inputs,
        s.rollbacks,
        s.resyncs,
        if alive { "alive" } else { "dead" },
        score.0,
        score.1,
        up / 1e3,
        down / 1e3,
    );
}
