//! The bases mode: bases, respawns around them, damage, elimination and the winner.

use gsim_client_core::{Controls, Session};
use gsim_core::{math, Body, Particle, ShipState};
use gsim_proto::{GameMode, PlayerId, BASE_HP};
use gsim_server::authority::DAMAGE;
use gsim_server::scenario::Scenario;
use gsim_testkit::*;

const LINK: Link = Link::new(15.0, 5.0, 0.0);

fn idle(_: usize, _: f64) -> Controls {
    Controls::default()
}

/// Nearly empty space, so that nothing but the bases pulls on the ships.
fn open_space() -> Scenario {
    Scenario {
        name: "open".into(),
        bodies: vec![Body { x: 3.0e11, y: 0.0, vx: 0.0, vy: 0.0, mass: 1.0e20, radius: 2.0e5 }],
        names: Vec::new(),
        spawn_r: (2.0e10, 3.0e10),
    }
}

/// ann (an operator) and bob, in a round of bases.
fn duel(seed: u64) -> (Sim, usize, usize, PlayerId, PlayerId) {
    let mut sim = Sim::new(open_space(), seed);
    sim.server.set_rounds(0, 600, None);
    sim.server.state.op("ann");
    sim.server.set_mode(GameMode::Bases);
    let a = sim.add_client("ann", LINK);
    let b = sim.add_client("bob", LINK);
    sim.run_with(1.5, idle);
    let (ann, bob) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    (sim, a, b, ann, bob)
}

fn hp(sim: &Sim, owner: PlayerId) -> u32 {
    sim.server.bases().iter().find(|b| b.owner == owner).unwrap().hp
}

fn base_at(sim: &Sim, owner: PlayerId) -> (usize, f64, f64) {
    let j = sim.server.bases().iter().find(|b| b.owner == owner).unwrap().slot as usize;
    (j, sim.server.massive.x[j], sim.server.massive.y[j])
}

/// A circular orbit of radius `r` around `owner`'s base.
fn orbit_around(sim: &Sim, owner: PlayerId, r: f64) -> Particle {
    let (j, x, y) = base_at(sim, owner);
    let v = (sim.server.rules.g * sim.server.massive.pull[j] / r).sqrt();
    Particle { x: x + r, y, vx: 0.0, vy: v }
}

fn place(sim: &mut Sim, who: PlayerId, p: Particle) {
    let old = sim.server.ship(who).unwrap();
    sim.server.place_ship(who, ShipState { p, ..old });
}

fn chat(s: &Session) -> Vec<String> {
    s.chat.iter().map(|c| c.text.clone()).collect()
}

fn in_sync(sim: &Sim) {
    for c in &sim.clients {
        assert_eq!(c.session.stats.hash_mismatches, 0);
        assert_eq!(c.session.stats.resyncs, 0);
    }
}

#[test]
fn every_player_gets_a_fixed_base_and_respawns_around_it() {
    let (mut sim, a, b, ann, bob) = duel(1);
    let bases = sim.server.bases();
    assert_eq!(bases.len(), 2);
    for base in &bases {
        let j = base.slot as usize;
        assert!(sim.server.massive.fixed[j] && sim.server.massive.mass[j] < 0.0 && sim.server.massive.pull[j] > 0.0);
        assert_eq!(base.hp, BASE_HP);
    }
    let start: Vec<(f64, f64)> = bases.iter().map(|b| (sim.server.massive.x[b.slot as usize], sim.server.massive.y[b.slot as usize])).collect();
    for who in [ann, bob] {
        let (j, x, y) = base_at(&sim, who);
        let s = sim.server.ship(who).unwrap().p;
        let r = ((s.x - x).powi(2) + (s.y - y).powi(2)).sqrt();
        assert!((20.0..60.0).contains(&(r / sim.server.massive.radius[j])), "ship {r} m from its base");
    }
    // Ships stay on their orbits and bases stay put.
    sim.run_with(20.0, idle);
    for (k, base) in bases.iter().enumerate() {
        assert_eq!((sim.server.massive.x[base.slot as usize], sim.server.massive.y[base.slot as usize]), start[k]);
    }
    for who in [ann, bob] {
        let (j, x, y) = base_at(&sim, who);
        let s = sim.server.ship(who).expect("still flying").p;
        let r = ((s.x - x).powi(2) + (s.y - y).powi(2)).sqrt();
        assert!((30.0..50.0).contains(&(r / sim.server.massive.radius[j])), "ship drifted to {r} m");
    }
    for i in [a, b] {
        let w = sim.clients[i].session.world.as_ref().unwrap();
        assert_eq!(w.mode, GameMode::Bases);
        assert_eq!(w.bases, sim.server.bases());
    }
    in_sync(&sim);
    assert!(sim.ships_agree().is_ok());
}

#[test]
fn shells_rams_and_sieges_wear_a_base_down() {
    let (shell, ram, siege) = DAMAGE;
    let (mut sim, a, _, ann, bob) = duel(2);

    // A shell dropped straight onto bob's base from outside the siege band.
    let r = 6.0e9;
    let p = orbit_around(&sim, bob, r);
    place(&mut sim, ann, p);
    let speed = 8000.0f64;
    let angle = math::dir_to_angle(-(speed * speed - p.vy * p.vy).sqrt(), -p.vy);
    let t0 = sim.time;
    sim.run_with(8.0, |i, t| if i == a && t < t0 + 0.2 { Controls { fire: Some((angle, speed as f32)), ..Controls::default() } } else { Controls::default() });
    assert_eq!(hp(&sim, bob), BASE_HP - shell);
    assert_eq!(hp(&sim, ann), BASE_HP, "nothing hit ann's base");

    // A ship on a close orbit around it lays siege.
    let before = hp(&sim, bob);
    let p = orbit_around(&sim, bob, 2.0e9);
    place(&mut sim, ann, p);
    sim.run_with(5.0, idle);
    let taken = before - hp(&sim, bob);
    assert!((4 * siege..=6 * siege).contains(&taken), "siege took {taken}");
    let w = sim.clients[a].session.world.as_ref().unwrap();
    assert_eq!(w.sieges, [(ann, bob)]);

    // Ramming costs the ship and some of the base.
    let before = hp(&sim, bob);
    let (_, x, y) = base_at(&sim, bob);
    place(&mut sim, ann, Particle { x: x + 2.0e9, y, vx: -20_000.0, vy: 0.0 });
    sim.run_with(4.0, idle);
    let taken = before - hp(&sim, bob);
    assert!((ram..ram + 2 * siege).contains(&taken), "ram took {taken}");
    assert_eq!(sim.server.score(ann).unwrap().1, 1, "the rammer died");
    sim.run_with(3.0, idle);
    assert!(sim.server.ship(ann).is_some(), "and came back: its own base stands");
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().bases, sim.server.bases());
    in_sync(&sim);
}

#[test]
fn without_a_base_the_next_death_is_the_last_and_the_survivor_wins() {
    let (mut sim, a, b, ann, bob) = duel(3);
    sim.server.hurt_base(bob, ann, BASE_HP);
    sim.run_with(1.0, idle);
    assert_eq!(hp(&sim, bob), 0);
    assert!(sim.server.ship(bob).is_some(), "bob still flies");
    assert!(sim.server.massive.alive[base_at(&sim, bob).0], "the hulk stays");
    for i in [a, b] {
        assert!(chat(&sim.clients[i].session).contains(&"ann destroyed bob's base".to_string()));
        assert_eq!(sim.clients[i].session.world.as_ref().unwrap().players[&ann].captures, 1);
    }

    sim.clients[a].session.send_chat("/kill bob");
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.spectating(bob), Some(true));
    for i in [a, b] {
        let w = sim.clients[i].session.world.as_ref().unwrap();
        assert!(w.players[&bob].spectating);
        assert_eq!(w.winner, Some(ann));
        assert!(w.next_round_tick.is_some());
        let lines = chat(&sim.clients[i].session);
        assert!(lines.contains(&"bob is out".to_string()) && lines.contains(&"ann wins the round".to_string()), "{lines:?}");
    }
    // The next round brings everyone back, with new bases.
    let round = sim.server.round();
    sim.run_with(12.0, idle);
    assert_eq!(sim.server.round(), round + 1);
    assert_eq!(sim.server.spectating(bob), Some(false));
    assert!(sim.server.ship(bob).is_some() && hp(&sim, bob) == BASE_HP);
    in_sync(&sim);
}

#[test]
fn operators_bring_players_back_even_without_a_base() {
    let (mut sim, a, _, ann, bob) = duel(4);
    sim.add_client("cat", LINK);
    sim.run_with(1.5, idle);
    sim.server.hurt_base(bob, ann, BASE_HP);
    sim.clients[a].session.send_chat("/kill bob");
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.spectating(bob), Some(true));
    assert!(sim.server.bases().len() == 2 && sim.server.spectating(sim.player_id(2).unwrap()) == Some(true), "cat only watches, ann and bob still play");

    sim.clients[a].session.send_chat("/gamemode play bob");
    sim.run_with(1.0, idle);
    let (j, x, y) = base_at(&sim, bob);
    let s = sim.server.ship(bob).expect("back in the game").p;
    let r = ((s.x - x).powi(2) + (s.y - y).powi(2)).sqrt();
    assert!(r < 60.0 * sim.server.massive.radius[j], "around the hulk of the old base");
    in_sync(&sim);
}

#[test]
fn late_players_watch_and_returning_owners_get_their_base_back() {
    let (mut sim, a, b, _, bob) = duel(5);
    let round = sim.server.round();
    let cat = sim.add_client("cat", LINK);
    sim.run_with(1.5, idle);
    let cat_id = sim.player_id(cat).unwrap();
    assert_eq!(sim.server.round(), round);
    assert_eq!(sim.server.spectating(cat_id), Some(true));
    assert!(sim.clients[cat].session.world.as_ref().unwrap().players[&cat_id].spectating);

    // bob drops out and comes back: the base is still his, and he plays on.
    sim.disconnect(b);
    sim.run_with(0.5, idle);
    let bob2 = sim.add_client("bob", LINK);
    sim.run_with(1.5, idle);
    let new_bob = sim.player_id(bob2).unwrap();
    assert_ne!(new_bob, bob);
    assert_eq!(sim.server.spectating(new_bob), Some(false));
    assert!(sim.server.ship(new_bob).is_some());
    assert!(sim.server.bases().iter().any(|base| base.owner == new_bob));
    assert_eq!(sim.server.round(), round);
    assert!(sim.clients[a].session.world.as_ref().unwrap().base_of(new_bob).is_some());
}

#[test]
fn the_mode_command_switches_at_once() {
    let mut sim = Sim::new(open_space(), 6);
    sim.server.state.op("ann");
    let a = sim.add_client("ann", LINK);
    sim.add_client("bob", LINK);
    sim.run_with(1.5, idle);
    assert!(sim.server.bases().is_empty());
    sim.clients[a].session.send_chat("/mode bases");
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.mode(), GameMode::Bases);
    assert_eq!(sim.server.bases().len(), 2);
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().bases.len(), 2);
    sim.clients[a].session.send_chat("/mode targets");
    sim.run_with(1.0, idle);
    assert!(sim.server.bases().is_empty());
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().mode, GameMode::Targets);
}
