//! UDP transport for [`Session`].

use crate::session::{Controls, Session, SessionConfig};
use gsim_proto::*;
use renet::RenetClient;
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant, SystemTime};

pub struct NetClient {
    pub session: Session,
    client: RenetClient,
    transport: NetcodeClientTransport,
    start: Instant,
    last: Instant,
    held: Vec<ClientMsg>,
}

pub fn resolve(addr: &str) -> Result<SocketAddr, String> {
    let with_port = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") };
    with_port
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {with_port}: {e}"))?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| format!("no IPv4 address for {with_port}"))
}

impl NetClient {
    pub fn connect(server: SocketAddr, cfg: SessionConfig) -> Result<Self, String> {
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
        let auth = ClientAuthentication::Unsecure {
            protocol_id: netcode_protocol_id(),
            client_id: now.as_nanos() as u64 ^ (std::process::id() as u64).rotate_left(32),
            server_addr: server,
            user_data: None,
        };
        let transport = NetcodeClientTransport::new(now, auth, socket).map_err(|e| e.to_string())?;
        let t = Instant::now();
        Ok(Self {
            session: Session::new(cfg),
            client: RenetClient::new(connection_config()),
            transport,
            start: t,
            last: t,
            held: Vec::new(),
        })
    }

    /// Seconds since this client was created (the session's clock).
    pub fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    pub fn disconnect_reason(&self) -> Option<String> {
        if self.client.is_disconnected() {
            let r = self.transport.disconnect_reason().map(|r| r.to_string());
            Some(r.or_else(|| self.client.disconnect_reason().map(|r| r.to_string())).unwrap_or_else(|| "disconnected".into()))
        } else {
            None
        }
    }

    pub fn bytes_per_sec(&self) -> (f64, f64) {
        (self.client.bytes_sent_per_sec(), self.client.bytes_received_per_sec())
    }

    /// Pump the network and advance the session. Call once per frame.
    pub fn update(&mut self, controls: Controls) {
        let t = Instant::now();
        let dt: Duration = t - self.last;
        self.last = t;
        self.client.update(dt);
        let _ = self.transport.update(dt, &mut self.client);
        let now = self.now();
        if self.client.is_connected() {
            for channel in [CH_RELIABLE, CH_UNRELIABLE] {
                while let Some(bytes) = self.client.receive_message(channel) {
                    if let Some(msg) = decode::<ServerMsg>(&bytes) {
                        self.session.handle(msg, now);
                    }
                }
            }
        }
        self.session.update(now, controls);
        self.held.extend(self.session.drain_out());
        if self.client.is_connected() {
            for msg in self.held.drain(..) {
                let channel = if msg.reliable() { CH_RELIABLE } else { CH_UNRELIABLE };
                self.client.send_message(channel, encode(&msg));
            }
            let _ = self.transport.send_packets(&mut self.client);
        }
    }

    pub fn disconnect(&mut self) {
        self.transport.disconnect();
    }
}
