//! For players in a browser, which can open neither a UDP socket nor a file: one TCP port
//! that hands out the web client's files and carries the game over WebSockets, each message
//! as one binary frame. A WebSocket delivers everything in order, so nothing more is needed
//! on top of it.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

/// Connections here are numbered from 1. (Those over UDP have numbers their clients pick
/// from the clock: far larger.)
pub type Conn = u64;

pub enum Event {
    Connected(Conn, IpAddr),
    Message(Conn, Vec<u8>),
    Disconnected(Conn),
}

/// A socket whose handshake is done, and where it comes from.
type Ready = (WebSocket<TcpStream>, IpAddr);

struct Peer {
    conn: Conn,
    socket: WebSocket<TcpStream>,
    /// When it last said anything.
    heard: Instant,
    gone: bool,
}

pub struct WebServer {
    listener: TcpListener,
    dir: Option<PathBuf>,
    /// Sockets whose handshake is done, from the threads that did it.
    ready: (Sender<Ready>, Receiver<Ready>),
    peers: Vec<Peer>,
    next: Conn,
    max_clients: usize,
}

/// A browser has to wait this long at most for the start of an answer.
const PATIENCE: Duration = Duration::from_secs(5);
/// A client says something several times a second. One silent for this long is gone,
/// whatever its socket says.
const SILENCE: Duration = Duration::from_secs(20);

impl WebServer {
    /// Listen on `bind`; `dir` holds the web client's files, if they are to be handed out.
    pub fn bind(bind: SocketAddr, dir: Option<PathBuf>, max_clients: usize) -> Result<Self, String> {
        let listener = TcpListener::bind(bind).map_err(|e| format!("bind {bind} (web): {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Self { listener, dir, ready: channel(), peers: Vec::new(), next: 1, max_clients })
    }

    /// What has happened since the last call. Never waits.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok((stream, _)) = self.listener.accept() {
            // Reading the request may take a while: not here.
            let (dir, ready) = (self.dir.clone(), self.ready.0.clone());
            let _ = std::thread::Builder::new().name("gsim-web".into()).spawn(move || answer(stream, dir.as_deref(), ready));
        }
        while let Ok((mut socket, ip)) = self.ready.1.try_recv() {
            if self.peers.len() >= self.max_clients {
                let _ = socket.close(None);
                continue;
            }
            self.peers.push(Peer { conn: self.next, socket, heard: Instant::now(), gone: false });
            events.push(Event::Connected(self.next, ip));
            self.next += 1;
        }
        for peer in &mut self.peers {
            while !peer.gone {
                match peer.socket.read() {
                    Ok(Message::Binary(bytes)) => {
                        peer.heard = Instant::now();
                        events.push(Event::Message(peer.conn, bytes.to_vec()));
                    }
                    Ok(Message::Close(_)) => peer.gone = true,
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => peer.gone = true,
                }
            }
            peer.gone |= peer.heard.elapsed() > SILENCE;
        }
        events.extend(self.peers.iter().filter(|peer| peer.gone).map(|peer| Event::Disconnected(peer.conn)));
        self.peers.retain(|peer| !peer.gone);
        events
    }

    pub fn is_connected(&self, conn: Conn) -> bool {
        self.peers.iter().any(|peer| peer.conn == conn && !peer.gone)
    }

    /// Queue a message; [`WebServer::flush`] sends what is queued.
    pub fn send(&mut self, conn: Conn, bytes: &[u8]) {
        if let Some(peer) = self.peers.iter_mut().find(|peer| peer.conn == conn && !peer.gone) {
            peer.gone = !sent(peer.socket.write(Message::binary(bytes.to_vec())));
        }
    }

    pub fn flush(&mut self) {
        for peer in self.peers.iter_mut().filter(|peer| !peer.gone) {
            peer.gone = !sent(peer.socket.flush());
        }
    }

    /// Close a connection. The next [`WebServer::poll`] reports it gone.
    pub fn close(&mut self, conn: Conn) {
        if let Some(peer) = self.peers.iter_mut().find(|peer| peer.conn == conn) {
            let _ = peer.socket.close(None);
            let _ = peer.socket.flush();
            peer.gone = true;
        }
    }

    pub fn close_all(&mut self) {
        for peer in &mut self.peers {
            let _ = peer.socket.close(None);
            let _ = peer.socket.flush();
        }
    }
}

/// Whether a write went through, or will once the socket has room again.
fn sent(result: Result<(), tungstenite::Error>) -> bool {
    match result {
        Ok(()) => true,
        Err(tungstenite::Error::Io(e)) => e.kind() == ErrorKind::WouldBlock,
        Err(_) => false,
    }
}

/// Deal with one new connection: a request for a file is answered and closed, one for a
/// WebSocket is handed back once the handshake is done.
fn answer(mut stream: TcpStream, dir: Option<&Path>, ready: Sender<Ready>) {
    let Ok(ip) = stream.peer_addr().map(|a| a.ip()) else { return };
    let _ = (stream.set_nonblocking(false), stream.set_read_timeout(Some(PATIENCE)), stream.set_write_timeout(Some(PATIENCE)), stream.set_nodelay(true));
    // Look at the request without taking it: a WebSocket handshake wants to read it itself.
    let mut head = [0u8; 4096];
    let mut seen = 0;
    for _ in 0..500 {
        seen = stream.peek(&mut head).unwrap_or(0);
        if seen == head.len() || head[..seen].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let request = String::from_utf8_lossy(&head[..seen]).to_ascii_lowercase();
    if request.lines().any(|line| line.starts_with("upgrade:") && line.contains("websocket")) {
        // A client sends a few small messages; the world's snapshot is the largest we do.
        let config = WebSocketConfig::default().max_message_size(Some(64 * 1024)).max_frame_size(Some(64 * 1024)).max_write_buffer_size(32 * 1024 * 1024);
        if let Ok(socket) = tungstenite::accept_with_config(stream, Some(config)) {
            if socket.get_ref().set_nonblocking(true).is_ok() {
                let _ = ready.send((socket, ip));
            }
        }
        return;
    }
    // Take the request off the socket, then answer it.
    let mut taken = vec![0u8; seen];
    let _ = stream.read_exact(&mut taken);
    let path = request.strip_prefix("get /").and_then(|rest| rest.split([' ', '?']).next());
    let file = match (dir, path) {
        (Some(dir), Some("")) => Some(dir.join("index.html")),
        // Only the files in the directory itself, by name.
        (Some(dir), Some(name)) if name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) && !name.starts_with('.') => Some(dir.join(name)),
        _ => None,
    };
    let _ = match file.as_ref().and_then(|file| Some((file, std::fs::read(file).ok()?))) {
        Some((file, body)) => {
            let kind = match file.extension().and_then(|e| e.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "text/javascript",
                Some("wasm") => "application/wasm",
                Some("png") => "image/png",
                _ => "application/octet-stream",
            };
            stream
                .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n", body.len()).as_bytes())
                .and_then(|()| stream.write_all(&body))
        }
        None => stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    };
    let _ = stream.flush();
}
