//! Special bodies in the bases mode: where they appear, what they give, and their comeback.

use gsim_client_core::{Controls, Session};
use gsim_core::{math, Body, Particle, ShipState};
use gsim_proto::{GameMode, PlayerId, SpecialKind, BASE_HP};
use gsim_server::scenario::{radius_from_mass, Scenario};
use gsim_testkit::*;

const LINK: Link = Link::new(15.0, 5.0, 0.0);

fn idle(_: usize, _: f64) -> Controls {
    Controls::default()
}

/// Six planets near the centre, far too light to move much during a test.
fn planets() -> Scenario {
    let bodies = (0..6)
        .map(|k| {
            let a = k as f64 * std::f64::consts::TAU / 6.0;
            Body { x: 5.0e9 * a.cos(), y: 5.0e9 * a.sin(), vx: 0.0, vy: 0.0, mass: 1.0e25, radius: radius_from_mass(1.0e25) }
        })
        .collect();
    Scenario { name: "planets".into(), bodies, names: Vec::new(), spawn_r: (2.0e10, 3.0e10) }
}

/// ann (an operator) and bob, in a round of bases among the planets.
fn duel(seed: u64) -> (Sim, usize, usize, PlayerId, PlayerId) {
    let mut sim = Sim::new(planets(), seed);
    sim.server.set_rounds(0, 600, None);
    sim.server.state.op("ann");
    sim.server.set_mode(GameMode::Bases);
    let a = sim.add_client("ann", LINK);
    let b = sim.add_client("bob", LINK);
    sim.run_with(1.5, idle);
    let (ann, bob) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    (sim, a, b, ann, bob)
}

fn slot_of(sim: &Sim, kind: SpecialKind) -> Option<u32> {
    sim.server.specials().iter().find(|s| s.kind == kind).map(|s| s.slot)
}

fn place(sim: &mut Sim, who: PlayerId, p: Particle) {
    let old = sim.server.ship(who).unwrap();
    sim.server.place_ship(who, ShipState { p, ..old });
}

/// Put `who` on a circular orbit 20 radii above the special of this kind and hold it.
fn take(sim: &mut Sim, who: PlayerId, kind: SpecialKind) -> u32 {
    let slot = slot_of(sim, kind).expect("there is one");
    let m = &sim.server.massive;
    let j = slot as usize;
    let r = 20.0 * m.radius[j];
    let v = (sim.server.rules.g * m.pull[j] / r).sqrt();
    let p = Particle { x: m.x[j] + r, y: m.y[j], vx: m.vx[j], vy: m.vy[j] + v };
    place(sim, who, p);
    sim.run_with(5.0, idle);
    slot
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
fn one_of_each_kind_appears_on_a_planet() {
    let (sim, a, b, _, _) = duel(1);
    let specials = sim.server.specials();
    for kind in SpecialKind::ALL {
        assert_eq!(specials.iter().filter(|s| s.kind == kind).count(), 1, "{kind:?}");
    }
    for s in &specials {
        let j = s.slot as usize;
        assert!(sim.server.massive.alive[j] && !sim.server.massive.fixed[j] && sim.server.massive.mass[j] > 0.0);
    }
    let slots: Vec<u32> = specials.iter().map(|s| s.slot).collect();
    assert!((1..slots.len()).all(|k| !slots[..k].contains(&slots[k])), "all on different bodies");
    for i in [a, b] {
        assert_eq!(sim.clients[i].session.world.as_ref().unwrap().specials, specials);
    }
}

#[test]
fn the_armoury_doubles_the_shells_and_the_rate_until_the_ship_is_lost() {
    let (mut sim, a, b, ann, _) = duel(2);
    take(&mut sim, ann, SpecialKind::Armoury);
    let mag = sim.server.magazine(ann).unwrap();
    let rules = sim.server.rules.clone();
    assert!(mag.quick && mag.max == 2 * rules.shell_max, "{mag:?}");
    assert_eq!(mag.cooldown(&rules), rules.shell_cooldown_ticks as u64 / 2);
    for i in [a, b] {
        let w = sim.clients[i].session.world.as_ref().unwrap();
        assert_eq!(w.players[&ann].magazine, mag);
        assert!(chat(&sim.clients[i].session).contains(&"ann raided an armoury".to_string()));
    }
    // Firing as fast as allowed: twice as many shells go out as without the armoury.
    let before = sim.server.shell_count();
    sim.run_with(3.0, |i, _| if i == a { Controls { fire: Some((0, 8000.0)), ..Controls::default() } } else { Controls::default() });
    let fired = sim.server.shell_count() - before;
    assert!((5..=7).contains(&fired), "{fired} shells in 3 s");

    // Gone from that body; another armoury turns up some 20 s later.
    assert!(slot_of(&sim, SpecialKind::Armoury).is_none());
    sim.run_with(18.0, idle);
    assert!(slot_of(&sim, SpecialKind::Armoury).is_some());

    sim.clients[a].session.send_chat("/kill ann");
    sim.run_with(7.0, idle);
    assert!(!sim.server.magazine(ann).unwrap().quick, "a new ship has the usual guns");
    assert!(!sim.clients[b].session.world.as_ref().unwrap().players[&ann].magazine.quick);
    in_sync(&sim);
}

#[test]
fn armour_stops_one_shell() {
    let (mut sim, a, b, ann, bob) = duel(3);
    take(&mut sim, ann, SpecialKind::Armour);
    assert_eq!(sim.server.armoured(ann), Some(true));
    assert!(sim.clients[b].session.world.as_ref().unwrap().players[&ann].armour);

    // Out in empty space, bob shoots at ann twice.
    place(&mut sim, ann, Particle { x: 2.0e11, y: 0.0, vx: 0.0, vy: 0.0 });
    place(&mut sim, bob, Particle { x: 2.0e11 + 1.0e9, y: 0.0, vx: 0.0, vy: 0.0 });
    sim.run_with(0.05, idle);
    assert!(sim.clients[b].session.world.as_ref().unwrap().players[&ann].armour, "moving a ship keeps its armour");
    let west = math::dir_to_angle(-1.0, 0.0);
    let t0 = sim.time;
    let shoot = |i: usize, t: f64| if i == b && (t < t0 + 0.1 || (t0 + 1.2..t0 + 1.3).contains(&t)) { Controls { fire: Some((west, 8000.0)), ..Controls::default() } } else { Controls::default() };
    sim.run_with(2.0, shoot);
    assert_eq!(sim.server.armoured(ann), Some(false));
    assert!(sim.server.ship(ann).is_some(), "the armour took the first shell");
    assert!(chat(&sim.clients[a].session).contains(&"ann's armour stopped a shell".to_string()));
    sim.run_with(2.0, shoot);
    assert!(sim.server.ship(ann).is_none(), "the second one gets through");
    assert_eq!(sim.server.score(bob).unwrap().0, 1);
    in_sync(&sim);
}

#[test]
fn the_shipyard_repairs_the_base() {
    let (mut sim, a, _, ann, bob) = duel(4);
    sim.server.hurt_base(ann, bob, BASE_HP / 2);
    sim.run_with(0.5, idle);
    take(&mut sim, ann, SpecialKind::Shipyard);
    let hp = sim.server.bases().iter().find(|b| b.owner == ann).unwrap().hp;
    assert_eq!(hp, BASE_HP);
    sim.run_with(1.0, idle);
    let w = sim.clients[a].session.world.as_ref().unwrap();
    assert_eq!(w.base_of(ann).unwrap().hp, BASE_HP);
    in_sync(&sim);
}

#[test]
fn more_players_more_specials() {
    let mut sim = Sim::new(planets(), 5);
    sim.server.set_rounds(0, 600, None);
    sim.server.set_mode(GameMode::Bases);
    for name in ["ann", "bob", "cat", "dan"] {
        sim.add_client(name, LINK);
        sim.run_with(1.0, idle);
    }
    sim.run_with(1.0, idle);
    // cat and dan joined a round under way: they watch it.
    assert_eq!(sim.server.bases().len(), 2);
    assert_eq!(sim.server.specials().len(), 3);
    // In the next round all four play.
    sim.server.set_mode(GameMode::Bases);
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.bases().len(), 4);
    let specials = sim.server.specials();
    for kind in SpecialKind::ALL {
        assert_eq!(specials.iter().filter(|s| s.kind == kind).count(), 2, "{kind:?} with four players");
    }
}
