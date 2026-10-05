//! Identity, name ownership, bans and the whitelist.

use gsim_client_core::Controls;
use gsim_proto::{ClientMsg, Identity, PROTOCOL_VERSION};
use gsim_server::state::ServerState;
use gsim_testkit::*;

fn idle(_: usize, _: f64) -> Controls {
    Controls::default()
}

const LINK: Link = Link::new(10.0, 0.0, 0.0);

fn rejected(sim: &Sim, client: usize) -> Option<&str> {
    sim.clients[client].session.rejected.as_deref()
}

#[test]
fn a_name_belongs_to_the_first_key_that_uses_it() {
    let mut sim = Sim::new(quiet_scenario(), 1);
    let ann = Identity::generate();
    let a = sim.add_client_as("Ann", ann.clone(), LINK);
    sim.run_with(1.0, idle);
    assert!(sim.clients[a].session.joined());

    // Someone else wants the same name, even with different capitalisation, even after
    // the owner has left: no.
    let thief = sim.add_client_as("ann", Identity::generate(), LINK);
    sim.run_with(1.0, idle);
    assert!(!sim.clients[thief].session.joined());
    assert_eq!(rejected(&sim, thief), Some("the name \"Ann\" belongs to another player on this server"));
    sim.disconnect(a);
    let thief2 = sim.add_client_as("Ann", Identity::generate(), LINK);
    sim.run_with(1.0, idle);
    assert!(!sim.clients[thief2].session.joined());

    // The owner comes back and is let in, under the registered spelling.
    let back = sim.add_client_as("ANN", ann, LINK);
    sim.run_with(1.0, idle);
    assert!(sim.clients[back].session.joined());
    let w = sim.clients[back].session.world.as_ref().unwrap();
    assert_eq!(w.me().unwrap().name, "Ann");
}

#[test]
fn the_same_player_joining_again_replaces_the_old_connection() {
    let mut sim = Sim::new(quiet_scenario(), 2);
    let ann = Identity::generate();
    let first = sim.add_client_as("ann", ann.clone(), LINK);
    sim.run_with(1.0, idle);
    // The first connection is still up (think: the game crashed and was restarted).
    let second = sim.add_client_as("ann", ann, LINK);
    sim.run_with(1.0, idle);
    assert!(sim.clients[second].session.joined(), "{:?}", rejected(&sim, second));
    assert_eq!(rejected(&sim, first), Some("you joined from another connection"));
    assert_eq!(sim.server.player_count(), 1);
    assert!(sim.player_id(first).is_none() && sim.player_id(second).is_some());
}

#[test]
fn a_forged_signature_is_refused() {
    let mut sim = Sim::new(quiet_scenario(), 3);
    // Speak the protocol by hand: announce a key we do not hold, then sign with another.
    let (claimed, actual) = (Identity::generate(), Identity::generate());
    let conn = 777;
    sim.server.connected(conn, None);
    let hello = ClientMsg::Hello { protocol: PROTOCOL_VERSION, golden: gsim_core::selftest::GOLDEN, name: "mallory".into(), key: claimed.public() };
    sim.server.handle(conn, hello, 0.0);
    let nonce = sim
        .server
        .drain_out()
        .into_iter()
        .find_map(|o| match o.msg {
            gsim_proto::ServerMsg::Challenge { nonce } => Some(nonce),
            _ => None,
        })
        .expect("server challenges the announced key");
    sim.server.handle(conn, ClientMsg::Auth { signature: actual.sign(&nonce, "mallory") }, 0.0);
    let out = sim.server.drain_out();
    assert!(out.iter().any(|o| matches!(&o.msg, gsim_proto::ServerMsg::Reject { reason } if reason == "identity check failed")));
    assert_eq!(sim.server.player_count(), 0);
    assert!(sim.server.state.record("mallory").is_none(), "a failed proof must not register the name");
    assert_eq!(sim.server.drain_kicks(), vec![conn]);
}

#[test]
fn bans_remove_players_and_keep_them_out() {
    let mut sim = Sim::new(quiet_scenario(), 4);
    let ann = Identity::generate();
    let a = sim.add_client_as("ann", ann.clone(), LINK);
    let b = sim.add_client("bob", LINK);
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.player_count(), 2);

    let dir = std::env::temp_dir().join(format!("gsim-auth-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Give the running server a state directory, then ban through a second handle on the
    // same files, exactly as `gsim-server admin ban` does.
    sim.server.state = ServerState::open(&dir, false).unwrap();
    sim.server.state.claim("ann", &ann.public(), Some(Sim::address(a))).unwrap();
    let mut admin = ServerState::open(&dir, false).unwrap();
    admin.ban("Ann", "being rude");
    sim.server.reload_state();
    sim.run_with(0.5, idle);
    assert_eq!(rejected(&sim, a), Some("you are banned from this server: being rude"));
    assert!(sim.player_id(a).is_none());
    assert!(sim.player_id(b).is_some(), "bob is untouched");

    let again = sim.add_client_as("ann", ann.clone(), LINK);
    sim.run_with(0.5, idle);
    assert_eq!(rejected(&sim, again), Some("you are banned from this server: being rude"));

    // Address bans: bob's address is taken from his last connection.
    assert!(admin.unban("ann"));
    admin.ban_ip(Sim::address(b), "");
    sim.server.reload_state();
    sim.run_with(0.5, idle);
    assert_eq!(rejected(&sim, b), Some("your address is banned from this server"));
    let ok = sim.add_client_as("ann", ann, LINK);
    sim.run_with(0.5, idle);
    assert!(sim.clients[ok].session.joined(), "unbanned players can return: {:?}", rejected(&sim, ok));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn whitelist_only_admits_listed_names() {
    let mut sim = Sim::new(quiet_scenario(), 5);
    sim.server.state.whitelist_enabled = true;
    sim.server.state.whitelist_add("Ann");
    let a = sim.add_client("ann", LINK);
    let b = sim.add_client("bob", LINK);
    sim.run_with(1.0, idle);
    assert!(sim.clients[a].session.joined());
    assert_eq!(rejected(&sim, b), Some("you are not on this server's whitelist"));
    assert!(sim.server.state.record("bob").is_none(), "refused players do not get to reserve names");
}

#[test]
fn state_survives_a_restart() {
    let dir = std::env::temp_dir().join(format!("gsim-state-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ann = Identity::generate();
    {
        let mut s = ServerState::open(&dir, false).unwrap();
        s.claim("Ann Droid", &ann.public(), Some("203.0.113.7".parse().unwrap())).unwrap();
        s.ban("Griefer", "no reason given");
        s.ban_ip("198.51.100.9".parse().unwrap(), "spam");
        s.whitelist_add("Ann Droid");
    }
    for file in ["players.txt", "whitelist.txt", "banned-players.txt", "banned-ips.txt"] {
        assert!(dir.join(file).is_file(), "{file} exists");
    }
    let mut s = ServerState::open(&dir, true).unwrap();
    let rec = s.record("ann droid").expect("name with a space survives");
    assert_eq!((rec.name.as_str(), rec.key, rec.last_ip), ("Ann Droid", ann.public(), Some("203.0.113.7".parse().unwrap())));
    assert!(s.claim("Ann Droid", &Identity::generate().public(), None).is_err());
    assert_eq!(s.name_ban("griefer"), Some("no reason given"));
    assert_eq!(s.ip_ban("198.51.100.9".parse().unwrap()), Some("spam"));
    assert!(s.whitelisted("ANN DROID") && !s.whitelisted("bob"));
    // Hand edits are picked up.
    assert!(!s.reload_if_changed());
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(dir.join("banned-players.txt"), "# edited by hand\nbob\ttesting\n").unwrap();
    assert!(s.reload_if_changed());
    assert_eq!(s.name_ban("Bob"), Some("testing"));
    assert_eq!(s.name_ban("griefer"), None);
    let _ = std::fs::remove_dir_all(&dir);
}
