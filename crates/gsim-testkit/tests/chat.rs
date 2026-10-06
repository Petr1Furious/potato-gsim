//! Chat, map markers and chat commands.

use gsim_client_core::{Controls, Session};
use gsim_core::objective::orbit_status;
use gsim_core::Particle;
use gsim_proto::ChatKind;
use gsim_testkit::*;

fn idle(_: usize, _: f64) -> Controls {
    Controls::default()
}

const LINK: Link = Link::new(15.0, 5.0, 0.0);

/// Texts of a session's chat lines of one kind.
fn lines(s: &Session, kind: fn(&ChatKind) -> bool) -> Vec<String> {
    s.chat.iter().filter(|c| kind(&c.kind)).map(|c| format!("{}{}", c.from.as_ref().map_or(String::new(), |f| format!("{f}: ")), c.text)).collect()
}

fn errors(s: &Session) -> Vec<String> {
    lines(s, |k| *k == ChatKind::Error)
}

fn system(s: &Session) -> Vec<String> {
    lines(s, |k| *k == ChatKind::System)
}

/// ann is an operator, bob and cat are not.
fn trio(seed: u64) -> (Sim, usize, usize, usize) {
    let mut sim = Sim::new(quiet_scenario(), seed);
    sim.server.state.op("ann");
    let a = sim.add_client("ann", LINK);
    let b = sim.add_client("bob", LINK);
    let c = sim.add_client("cat", LINK);
    sim.run_with(1.5, idle);
    (sim, a, b, c)
}

fn say(sim: &mut Sim, who: usize, text: &str) {
    sim.clients[who].session.send_chat(text);
    sim.run_with(0.4, idle);
}

#[test]
fn messages_reach_everyone_and_whispers_only_their_target() {
    let (mut sim, a, b, c) = trio(1);
    assert!(sim.clients[a].session.op && !sim.clients[b].session.op);
    say(&mut sim, b, "hello there");
    for i in [a, b, c] {
        assert_eq!(lines(&sim.clients[i].session, |k| *k == ChatKind::Say), ["bob: hello there"]);
    }
    // Events show up as system lines.
    sim.add_client("dan", LINK);
    sim.run_with(1.0, idle);
    for i in [a, b, c] {
        assert!(system(&sim.clients[i].session).iter().any(|l| l == "dan joined the game"), "{:?}", system(&sim.clients[i].session));
    }

    say(&mut sim, b, "/msg ann psst");
    say(&mut sim, a, "/r heard you");
    let private = |i: usize| lines(&sim.clients[i].session, |k| matches!(k, ChatKind::Private { .. }));
    assert_eq!(private(a), ["bob: psst", "bob: heard you"], "incoming from bob, then outgoing to bob");
    assert_eq!(private(b), ["ann: psst", "ann: heard you"]);
    assert!(private(c).is_empty());

    // Flooding is cut off.
    for _ in 0..12 {
        sim.clients[c].session.send_chat("spam");
    }
    sim.run_with(0.5, idle);
    let heard = lines(&sim.clients[a].session, |k| *k == ChatKind::Say).iter().filter(|l| *l == "cat: spam").count();
    assert!((1..=10).contains(&heard), "{heard}");
    assert!(errors(&sim.clients[c].session).contains(&"slow down".to_string()));
}

#[test]
fn only_operators_may_change_the_game() {
    let (mut sim, a, b, _) = trio(2);
    let hz = sim.server.rules.tick_hz as u64;
    sim.server.set_rounds(600 * hz, hz, None);
    let round = sim.server.round();
    say(&mut sim, b, "/round new");
    assert_eq!(errors(&sim.clients[b].session), ["/round is for operators"]);
    assert_eq!(sim.server.round(), round);
    say(&mut sim, b, "/nonsense");
    assert_eq!(errors(&sim.clients[b].session).last().unwrap(), "unknown command /nonsense (try /help)");

    say(&mut sim, a, "/round new");
    assert_eq!(sim.server.round(), round + 1);
    sim.run_with(1.0, idle);
    assert_eq!(sim.clients[b].session.world.as_ref().unwrap().round, round + 1);

    say(&mut sim, a, "/round time 30");
    let w = sim.clients[b].session.world.as_ref().unwrap();
    let left = w.round_end_tick.unwrap().saturating_sub(sim.server.tick());
    assert!(left > 25 * hz && left <= 30 * hz, "{left}");

    // /help lists more for operators than for others.
    say(&mut sim, a, "/help");
    say(&mut sim, b, "/help");
    let (ops, others) = (system(&sim.clients[a].session), system(&sim.clients[b].session));
    assert!(ops.iter().any(|l| l.starts_with("/tp ")) && !others.iter().any(|l| l.starts_with("/tp ")));
    assert!(others.iter().any(|l| l.starts_with("/msg ")));
}

#[test]
fn teleports_and_orbits() {
    let (mut sim, a, b, _) = trio(3);
    let (pa, pb) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    let ship = |sim: &Sim, p| sim.server.ship(p).unwrap().p;
    let dist = |p: Particle, q: Particle| ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt();

    say(&mut sim, a, "/tp bob");
    assert!(dist(ship(&sim, pa), ship(&sim, pb)) < 1.0e9, "next to bob");

    let before = ship(&sim, pa);
    // Checked right away: at 30 km/s the ship covers 4e7 m every tick.
    sim.clients[a].session.send_chat("/tp ~20Gm ~-1e10");
    sim.run_with(0.1, idle);
    let after = ship(&sim, pa);
    // The ship keeps flying meanwhile, hence the tolerance.
    assert!((after.x - before.x - 2.0e10).abs() < 5.0e8 && (after.y - before.y + 1.0e10).abs() < 5.0e8, "{before:?} -> {after:?}");

    say(&mut sim, a, "/tp bob ann");
    assert!(dist(ship(&sim, pa), ship(&sim, pb)) < 1.0e9, "bob was brought to ann");

    say(&mut sim, a, "/tp cat Star");
    let pc = sim.player_id(2).unwrap();
    let star = Particle::default();
    assert!(dist(ship(&sim, pc), star) < 200.0 * 7.0e8);

    say(&mut sim, a, "/orbit Star");
    let rules = sim.server.rules.clone();
    let o = orbit_status(&ship(&sim, pa), &star, 2.0e30, 7.0e8, &rules);
    assert!(o.ok && o.ecc < 0.05, "{o:?}");

    say(&mut sim, a, "/tp nobody");
    assert_eq!(errors(&sim.clients[a].session).last().unwrap(), "\"nobody\" is neither a player nor a body");
    say(&mut sim, a, "/tp");
    assert!(errors(&sim.clients[a].session).last().unwrap().starts_with("usage: /tp"));
    sim.run_with(1.0, idle);
    sim.ships_agree().unwrap();
}

#[test]
fn cheats_and_scores() {
    let (mut sim, a, b, _) = trio(4);
    let (pa, pb) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    let rules = sim.server.rules.clone();

    sim.run_with(2.0, |i, _| Controls { angle: 0, thrust: if i == 0 { 100 } else { 0 }, fire: None });
    assert!(sim.server.ship(pa).unwrap().fuel < rules.fuel_max_mmps);
    say(&mut sim, a, "/fuel");
    assert!(sim.server.ship(pa).unwrap().fuel > rules.fuel_max_mmps * 9 / 10);

    say(&mut sim, a, "/score bob 4 2");
    assert_eq!(sim.server.score(pb), Some((4, 0)));
    let w = sim.clients[a].session.world.as_ref().unwrap();
    assert_eq!(w.players[&pb].score(&w.rules), 4 + 2 * rules.capture_points);

    say(&mut sim, a, "/kill bob");
    assert!(sim.server.ship(pb).is_none());
    assert!(sim.clients[b].session.world.as_ref().unwrap().players[&pb].respawn_tick.is_some());
    // Anyone may respawn themselves, nobody else.
    say(&mut sim, b, "/respawn ann");
    assert_eq!(errors(&sim.clients[b].session).last().unwrap(), "only operators can respawn someone else");

    // God mode: a shell that would kill bob does nothing.
    sim.run_with(4.0, idle);
    say(&mut sim, a, "/god bob");
    assert!(system(&sim.clients[a].session).last().unwrap().contains("now immune"));
    let at = |x: f64| gsim_core::ShipState::new(Particle { x, y: 1.5e11, vx: 0.0, vy: 0.0 }, &rules);
    sim.server.place_ship(pa, at(0.0));
    sim.server.place_ship(pb, at(4.0e8));
    sim.run_with(0.5, idle);
    let deaths = sim.server.score(pb).unwrap().1;
    sim.run_with(0.3, |i, _| Controls { angle: 0, thrust: 0, fire: (i == 0).then_some((0, 8000.0)) });
    sim.run_with(1.5, idle);
    assert_eq!(sim.server.score(pb).unwrap().1, deaths, "the shell passed through");
    assert!(sim.server.ship(pb).is_some());
}

#[test]
fn moderation_from_chat() {
    let (mut sim, a, b, c) = trio(5);
    say(&mut sim, a, "/kick bob being loud");
    assert_eq!(sim.clients[b].session.rejected.as_deref(), Some("kicked: being loud"));
    assert_eq!(sim.server.player_count(), 2);

    say(&mut sim, a, "/op cat");
    assert!(sim.clients[c].session.op);
    say(&mut sim, c, "/ban ann swapping sides");
    assert_eq!(sim.clients[a].session.rejected.as_deref(), Some("you are banned from this server: swapping sides"));
    say(&mut sim, c, "/unban ann");
    say(&mut sim, c, "/deop cat");
    assert!(!sim.clients[c].session.op);
    say(&mut sim, c, "/list");
    assert_eq!(system(&sim.clients[c].session).last().unwrap(), "There are 1 players online: cat");

    say(&mut sim, c, "/op cat");
    assert_eq!(errors(&sim.clients[c].session).last().unwrap(), "/op is for operators");
}

#[test]
fn world_control() {
    let (mut sim, a, b, _) = trio(6);
    let hz = sim.server.rules.tick_hz as u64;
    sim.server.set_rounds(600 * hz, hz, None);
    sim.server.enable_objective();
    say(&mut sim, a, "/target star");
    assert_eq!(sim.server.target(), Some(0));
    assert_eq!(sim.clients[b].session.world.as_ref().unwrap().target, Some(0));

    say(&mut sim, a, "/preset solar 7");
    sim.run_with(1.5, idle);
    assert_eq!(sim.server.massive.len(), 10);
    let w = sim.clients[b].session.world.as_ref().unwrap();
    assert_eq!(w.preset, "solar");
    assert_eq!(w.names.get(&0).map(String::as_str), Some("Sun"));
    say(&mut sim, a, "/preset nowhere");
    assert!(errors(&sim.clients[a].session).last().unwrap().starts_with("unknown preset"));

    say(&mut sim, a, "/timescale 3600");
    sim.run_with(1.5, idle);
    assert_eq!(sim.clients[b].session.world.as_ref().unwrap().rules.time_scale(), 3600.0);
    assert_eq!(sim.clients[b].session.stats.hash_mismatches, 0);
    sim.ships_agree().unwrap();
}

#[test]
fn map_markers_reach_other_players() {
    let (mut sim, a, b, _) = trio(7);
    sim.clients[a].session.send_mark(1.0e11, -2.0e10);
    sim.clients[a].session.send_mark(5.0, 5.0); // too soon after the first: dropped
    sim.run_with(0.4, idle);
    let marks = &sim.clients[b].session.marks;
    assert_eq!(marks.len(), 1);
    assert_eq!((marks[0].name.as_str(), marks[0].x, marks[0].y), ("ann", 1.0e11, -2.0e10));
}
