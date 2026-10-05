//! Deterministic network laboratory: one authoritative server and any number of headless
//! clients on a virtual clock, joined by links with configurable latency, jitter and loss.
//! Every message goes through the real wire encoding.

use gsim_client_core::{Controls, Session, SessionConfig};
use gsim_core::{Body, GameRules};
use gsim_proto::{decode, encode, ClientMsg, Identity, PlayerId, ServerMsg};
use std::net::{IpAddr, Ipv4Addr};
use gsim_server::rng::Rng;
use gsim_server::{Authority, ConnId, Scenario};

/// One-way link characteristics (seconds; `loss` applies to unreliable messages only).
#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub latency: f64,
    pub jitter: f64,
    pub loss: f64,
}

impl Link {
    pub const fn new(latency_ms: f64, jitter_ms: f64, loss: f64) -> Self {
        Self { latency: latency_ms / 1e3, jitter: jitter_ms / 1e3, loss }
    }
}

struct InFlight {
    at: f64,
    order: u64,
    bytes: Vec<u8>,
}

pub struct SimClient {
    pub session: Session,
    pub controls: Controls,
    pub link: Link,
    conn: ConnId,
    /// The client's own clock is offset from virtual time: nothing may depend on shared clocks.
    skew: f64,
    up: Vec<InFlight>,
    down: Vec<InFlight>,
    up_reliable_at: f64,
    down_reliable_at: f64,
    pub connected: bool,
}

pub struct Sim {
    pub server: Authority,
    pub clients: Vec<SimClient>,
    pub time: f64,
    accum: f64,
    period: f64,
    rng: Rng,
    order: u64,
    /// Virtual seconds per [`Sim::run`] sub-step; clients update once per sub-step.
    pub frame: f64,
}

/// A nearly empty world: one distant star, so ships move almost inertially.
pub fn quiet_scenario() -> Scenario {
    Scenario {
        name: "quiet".into(),
        bodies: vec![Body { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, mass: 2.0e30, radius: 7.0e8 }],
        names: vec![(0, "Star".into())],
        spawn_r: (1.4e11, 1.6e11),
    }
}

impl Sim {
    pub fn new(scenario: Scenario, seed: u64) -> Self {
        let rules = GameRules::new(86400.0, 60);
        let period = 1.0 / rules.tick_hz as f64;
        Self {
            server: Authority::new(scenario, rules, seed),
            clients: Vec::new(),
            time: 0.0,
            accum: 0.0,
            period,
            rng: Rng::new(seed ^ 0xfeed),
            order: 0,
            frame: 1.0 / 120.0,
        }
    }

    pub fn add_client(&mut self, name: &str, link: Link) -> usize {
        self.add_client_as(name, Identity::insecure_from_label(name), link)
    }

    /// Join as `name` with a specific identity key (the plain `add_client` derives one from
    /// the name). Each client gets its own address, `10.0.0.<index + 1>`.
    pub fn add_client_as(&mut self, name: &str, identity: Identity, link: Link) -> usize {
        let i = self.clients.len();
        let conn = 100 + i as ConnId;
        self.server.connected(conn, Some(Self::address(i)));
        self.clients.push(SimClient {
            session: Session::new(SessionConfig { identity, ..SessionConfig::headless(name) }),
            controls: Controls::default(),
            link,
            conn,
            skew: 1000.0 * (i as f64 + 1.0) + 0.123,
            up: Vec::new(),
            down: Vec::new(),
            up_reliable_at: 0.0,
            down_reliable_at: 0.0,
            connected: true,
        });
        i
    }

    pub fn address(client: usize) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, client as u8 + 1))
    }

    pub fn player_id(&self, client: usize) -> Option<PlayerId> {
        self.server.player_of(self.clients[client].conn)
    }

    pub fn disconnect(&mut self, client: usize) {
        self.clients[client].connected = false;
        self.server.disconnect(self.clients[client].conn);
    }

    fn delivery(rng: &mut Rng, link: &Link, time: f64, reliable: bool, reliable_at: &mut f64) -> Option<f64> {
        let jitter = link.jitter * rng.f64();
        if reliable {
            // Ordered and lossless; a lost packet shows up as extra delay.
            let mut at = time + link.latency + jitter;
            if rng.f64() < link.loss {
                at += 0.1 + 2.0 * link.latency;
            }
            *reliable_at = reliable_at.max(at);
            Some(*reliable_at)
        } else if rng.f64() < link.loss {
            None
        } else {
            Some(time + link.latency + jitter)
        }
    }

    /// Advance virtual time by `seconds`.
    pub fn run(&mut self, seconds: f64) {
        let end = self.time + seconds;
        while self.time < end {
            self.time += self.frame;
            self.accum += self.frame;
            let time = self.time;

            // Client -> server deliveries.
            for c in &mut self.clients {
                c.up.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.order.cmp(&b.order)));
                let due = c.up.iter().take_while(|m| m.at <= time).count();
                for m in c.up.drain(..due) {
                    if let Some(msg) = decode::<ClientMsg>(&m.bytes) {
                        let frac = self.server.tick() as f64 + (self.accum / self.period).min(0.999);
                        self.server.handle(c.conn, msg, frac);
                    }
                }
            }
            while self.accum >= self.period {
                self.server.step();
                self.accum -= self.period;
            }
            // Refused connections are simply dropped by the transport in real life.
            self.server.drain_kicks();
            // Server -> client sends.
            for out in self.server.drain_out() {
                let bytes = encode(&out.msg);
                for conn in out.to {
                    let Some(c) = self.clients.iter_mut().find(|c| c.conn == conn && c.connected) else { continue };
                    let at = Self::delivery(&mut self.rng, &c.link, time, out.msg.reliable(), &mut c.down_reliable_at);
                    if let Some(at) = at {
                        self.order += 1;
                        c.down.push(InFlight { at, order: self.order, bytes: bytes.clone() });
                    }
                }
            }
            // Deliveries to clients, client frames, client sends.
            for c in &mut self.clients {
                if !c.connected {
                    continue;
                }
                let local = time + c.skew;
                c.down.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.order.cmp(&b.order)));
                let due = c.down.iter().take_while(|m| m.at <= time).count();
                for m in c.down.drain(..due) {
                    if let Some(msg) = decode::<ServerMsg>(&m.bytes) {
                        c.session.handle(msg, local);
                    }
                }
                c.session.update(local, c.controls);
                for msg in c.session.drain_out() {
                    let at = Self::delivery(&mut self.rng, &c.link, time, msg.reliable(), &mut c.up_reliable_at);
                    if let Some(at) = at {
                        self.order += 1;
                        c.up.push(InFlight { at, order: self.order, bytes: encode(&msg) });
                    }
                }
            }
        }
    }

    /// Run with a control script: `script(client_index, virtual_time)` each frame.
    pub fn run_with(&mut self, seconds: f64, mut script: impl FnMut(usize, f64) -> Controls) {
        let end = self.time + seconds;
        while self.time < end {
            for (i, c) in self.clients.iter_mut().enumerate() {
                c.controls = script(i, self.time);
            }
            let frame = self.frame;
            self.run(frame * 0.999);
        }
    }

    /// True if every client's replica of every ship equals the server's, bit for bit, at the
    /// server's current tick. Call after the world has been quiet for a moment.
    pub fn ships_agree(&self) -> Result<(), String> {
        let tick = self.server.tick();
        for (ci, c) in self.clients.iter().enumerate().filter(|c| c.1.connected) {
            let w = c.session.world.as_ref().ok_or(format!("client {ci} has no world"))?;
            for other in 0..self.clients.len() {
                let Some(pid) = self.player_id(other) else { continue };
                let server = self.server.ship(pid);
                let local = w.players.get(&pid).and_then(|p| p.ship.as_ref()).and_then(|s| s.at(tick)).copied();
                if server != local {
                    return Err(format!(
                        "client {ci} disagrees about player {pid} at tick {tick} (head {}):\n server {server:?}\n client {local:?}",
                        w.head
                    ));
                }
            }
        }
        Ok(())
    }
}
