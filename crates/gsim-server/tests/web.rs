//! A browser's way in: the same port hands out the web client and carries the game.

use gsim_proto::identity::Identity;
use gsim_proto::{decode, encode, ClientMsg, ServerMsg, PROTOCOL_VERSION};
use gsim_server::net::{run, ServerOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tungstenite::Message;

#[test]
fn a_browser_gets_the_page_and_joins_over_a_websocket() {
    let dir = std::env::temp_dir().join(format!("gsim-web-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<p>the game</p>").unwrap();
    // A free port: ask for one, let go of it, use it.
    let web: SocketAddr = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let opts = ServerOptions { bind: "127.0.0.1:0".parse().unwrap(), web: Some(web), web_dir: Some(dir.clone()), quiet: true, ..Default::default() };
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let server = std::thread::spawn(move || run(opts, flag).unwrap());
    let connect = || {
        let tried = (0..200).find_map(|_| {
            let stream = TcpStream::connect(web).ok();
            if stream.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            stream
        });
        tried.expect("the server listens")
    };

    let mut page = connect();
    page.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
    let mut got = String::new();
    page.read_to_string(&mut got).unwrap();
    assert!(got.starts_with("HTTP/1.1 200") && got.contains("text/html") && got.ends_with("<p>the game</p>"), "{got}");
    let mut missing = connect();
    missing.write_all(b"GET /../secret HTTP/1.1\r\n\r\n").unwrap();
    got.clear();
    missing.read_to_string(&mut got).unwrap();
    assert!(got.starts_with("HTTP/1.1 404"), "{got}");

    let (mut socket, _) = tungstenite::client(format!("ws://{web}/ws"), connect()).unwrap();
    let identity = Identity::generate();
    let hello = ClientMsg::Hello { protocol: PROTOCOL_VERSION, golden: gsim_core::selftest::GOLDEN, name: "browser".into(), key: identity.public(), color: [200, 200, 200] };
    socket.send(Message::binary(encode(&hello))).unwrap();
    fn next(socket: &mut tungstenite::WebSocket<TcpStream>) -> ServerMsg {
        loop {
            if let Message::Binary(bytes) = socket.read().unwrap() {
                return decode::<ServerMsg>(&bytes).expect("a message of the protocol");
            }
        }
    }
    let ServerMsg::Challenge { nonce } = next(&mut socket) else { panic!("no challenge") };
    let auth = ClientMsg::Auth { signature: identity.sign(&nonce, "browser") };
    socket.send(Message::binary(encode(&auth))).unwrap();
    let welcomed = (0..50).any(|_| matches!(next(&mut socket), ServerMsg::Welcome(_)));
    assert!(welcomed, "no welcome after the challenge was answered");

    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
