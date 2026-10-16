//! Teams in the bases mode: who plays with whom, and what a team shares.

use gsim_client_core::{Controls, Session};
use gsim_core::{Body, Particle, ShipState};
use gsim_proto::shop::Upgrade;
use gsim_proto::{GameMode, PlayerId, Purse, Roll, Twist, Ware, BASE_HP};
use gsim_server::scenario::Scenario;
use gsim_testkit::*;

const LINK: Link = Link::new(15.0, 5.0, 0.0);

fn idle(_: usize, _: f64) -> Controls {
    Controls::default()
}

fn open_space() -> Scenario {
    Scenario {
        name: "open".into(),
        bodies: vec![Body { x: 3.0e11, y: 0.0, vx: 0.0, vy: 0.0, mass: 1.0e20, radius: 2.0e5 }],
        names: Vec::new(),
        spawn_r: (2.0e10, 3.0e10),
    }
}

const NAMES: [&str; 4] = ["ann", "bob", "cat", "dan"];

/// Four players (ann an operator) in a round of bases in teams of two.
fn doubles(seed: u64) -> (Sim, Vec<PlayerId>) {
    let mut sim = Sim::new(open_space(), seed);
    sim.server.set_rounds(0, 600, None);
    sim.server.state.op("ann");
    sim.server.fix_roll(Roll { scrap: 100, mines: 0, twist: Twist::Plain, teams: 2 });
    sim.server.set_mode(GameMode::Bases);
    for name in NAMES {
        sim.add_client(name, LINK);
        sim.run_with(0.5, idle);
    }
    sim.clients[0].session.send_chat("/round new");
    sim.run_with(1.5, idle);
    let ids = (0..4).map(|i| sim.player_id(i).unwrap()).collect();
    (sim, ids)
}

fn place(sim: &mut Sim, who: PlayerId, p: Particle) {
    let old = sim.server.ship(who).unwrap();
    sim.server.place_ship(who, ShipState { p, ..old });
}

fn home_orbit(sim: &Sim, who: PlayerId, radii: f64) -> Particle {
    let j = sim.server.base_of_player(who).unwrap().slot as usize;
    let m = &sim.server.massive;
    let r = radii * m.radius[j];
    let v = (sim.server.rules.g * m.pull[j] / r).sqrt();
    Particle { x: m.x[j] + r, y: m.y[j], vx: 0.0, vy: v }
}

fn chat(s: &Session) -> Vec<String> {
    s.chat.iter().map(|c| c.text.clone()).collect()
}

#[test]
fn teams_of_two_share_a_base_a_bank_and_its_upgrades() {
    let (mut sim, ids) = doubles(1);
    assert_eq!(sim.server.bases().len(), 2);
    let team = |i: usize| sim.server.team(ids[i]).unwrap();
    let mate = (1..4).find(|i| team(*i) == team(0)).expect("ann has a partner");
    let rival = (1..4).find(|i| team(*i) != team(0)).unwrap();
    assert_eq!(sim.server.base_of_player(ids[0]), sim.server.base_of_player(ids[mate]));
    // Everybody sees the same teams.
    for c in &sim.clients {
        let w = c.session.world.as_ref().unwrap();
        assert!(w.in_teams());
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(w.team_of(*id), team(i));
        }
    }
    assert!(chat(&sim.clients[0].session).iter().any(|l| l.starts_with("Red: ") || l.contains("  ·  ")));
    // Banked by one, spent by the other; the base's upgrades are for both.
    sim.server.grant(ids[0], Purse { scrap: 50, crystals: 0 });
    sim.run_with(0.5, idle);
    assert_eq!(sim.clients[mate].session.world.as_ref().unwrap().players[&ids[mate]].bank.scrap, 50);
    assert_eq!(sim.server.purse(ids[rival]).unwrap().1.scrap, 0);
    let p = home_orbit(&sim, ids[mate], 60.0);
    place(&mut sim, ids[mate], p);
    sim.run_with(0.3, idle);
    sim.clients[mate].session.buy(Ware::Upgrade(Upgrade::Armor));
    sim.clients[mate].session.buy(Ware::Upgrade(Upgrade::Thrust));
    sim.run_with(0.5, idle);
    assert_eq!(sim.server.purse(ids[0]).unwrap().1.scrap, 50 - Upgrade::Armor.price(1).scrap - Upgrade::Thrust.price(1).scrap);
    assert_eq!(sim.server.purse(ids[0]).unwrap().2.tier(Upgrade::Armor), 1, "the base's, so ann's too");
    assert_eq!(sim.server.purse(ids[0]).unwrap().2.tier(Upgrade::Thrust), 1, "the ship upgrades too");
    let rules = sim.server.rules.clone();
    assert_eq!(sim.server.ship(ids[0]).unwrap().kit.burn, gsim_core::Kit::of(&rules).burn * 3 / 2, "ann flies with it at once");
    assert_eq!(sim.clients[0].session.world.as_ref().unwrap().players[&ids[0]].upgrades.tier(Upgrade::Armor), 1);
    sim.ships_agree().unwrap();
}

#[test]
fn partners_do_not_besiege_their_own_base_and_the_last_team_wins() {
    let (mut sim, ids) = doubles(2);
    let team = |sim: &Sim, i: usize| sim.server.team(ids[i]).unwrap();
    let mate = (1..4).find(|i| team(&sim, *i) == team(&sim, 0)).unwrap();
    let rivals: Vec<usize> = (1..4).filter(|i| team(&sim, *i) != team(&sim, 0)).collect();
    // ann circles her own base close in: no siege.
    let p = home_orbit(&sim, ids[0], 10.0);
    place(&mut sim, ids[0], p);
    sim.run_with(2.0, idle);
    assert_eq!(sim.server.base_of_player(ids[0]).unwrap().hp, BASE_HP);
    // ann's team loses its base, then both ships: the other team wins.
    let rival = ids[rivals[0]];
    sim.server.hurt_base(ids[0], rival, BASE_HP);
    sim.run_with(0.5, idle);
    for i in [0, mate] {
        sim.clients[0].session.send_chat(&format!("/kill {}", NAMES[i]));
        sim.run_with(0.5, idle);
        assert_eq!(sim.server.spectating(ids[i]), Some(true), "{} is out", NAMES[i]);
    }
    let w = sim.clients[rivals[1]].session.world.as_ref().unwrap();
    assert_eq!(w.winner, Some(team(&sim, rivals[0])));
    let label = w.team_label(team(&sim, rivals[0]));
    assert!(chat(&sim.clients[0].session).contains(&format!("{label} wins the round")));
}

#[test]
fn teams_are_rolled_only_with_enough_players_and_operators_can_pick() {
    let mut sim = Sim::new(open_space(), 3);
    sim.server.set_rounds(0, 600, None);
    sim.server.state.op("ann");
    sim.server.set_mode(GameMode::Bases);
    sim.add_client("ann", LINK);
    sim.add_client("bob", LINK);
    sim.run_with(1.5, idle);
    for _ in 0..8 {
        sim.clients[0].session.send_chat("/round new");
        sim.run_with(1.0, idle);
        assert_eq!(sim.server.roll().teams, 1, "two players never make teams");
    }
    for name in ["cat", "dan"] {
        sim.add_client(name, LINK);
        sim.run_with(0.5, idle);
    }
    let mut sizes = Vec::new();
    for _ in 0..12 {
        sim.clients[0].session.send_chat("/round new");
        sim.run_with(1.0, idle);
        sizes.push(sim.server.roll().teams);
        assert_eq!(sim.server.bases().len(), if sim.server.roll().teams == 2 { 2 } else { 4 });
    }
    assert!(sizes.contains(&1) && sizes.contains(&2), "{sizes:?}");
    assert!(!sizes.contains(&3), "three a side takes six");
    // ann and dan together, by hand.
    for line in ["/team ann 2", "/team dan 2", "/mode bases teams=2"] {
        sim.clients[0].session.send_chat(line);
        sim.run_with(1.0, idle);
    }
    let id = |i: usize| sim.player_id(i).unwrap();
    assert_eq!(sim.server.team(id(0)), Some(2));
    assert_eq!(sim.server.team(id(3)), Some(2));
    assert_eq!(sim.server.team(id(1)), Some(1));
}
