//! The shop's supplies and base upgrades in the bases mode.

use gsim_client_core::{Controls, Session};
use gsim_core::{math, Body, Particle, ShipState};
use gsim_proto::shop::Upgrade;
use gsim_proto::{Roll, Twist, GameMode, PieceKind, PlayerId, Purse, Supply, Ware, BASE_HP, HOME_RADII, REPAIR_HP};
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

/// ann and bob in a round of bases, ann with plenty in the bank.
fn duel(seed: u64) -> (Sim, usize, usize, PlayerId, PlayerId) {
    let mut sim = Sim::new(open_space(), seed);
    sim.server.set_rounds(0, 600, None);
    sim.server.fix_roll(Roll { scrap: 100, mines: 0, twist: Twist::Plain, teams: 1 });
    sim.server.set_mode(GameMode::Bases);
    let a = sim.add_client("ann", LINK);
    let b = sim.add_client("bob", LINK);
    sim.run_with(1.5, idle);
    let (ann, bob) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    sim.server.grant(ann, Purse { scrap: 500, crystals: 20 });
    (sim, a, b, ann, bob)
}

fn place(sim: &mut Sim, who: PlayerId, p: Particle) {
    let old = sim.server.ship(who).unwrap();
    sim.server.place_ship(who, ShipState { p, ..old });
}

fn base_at(sim: &Sim, owner: PlayerId) -> (usize, f64, f64) {
    let j = sim.server.base_of_player(owner).unwrap().slot as usize;
    (j, sim.server.massive.x[j], sim.server.massive.y[j])
}

fn hp(sim: &Sim, owner: PlayerId) -> u32 {
    sim.server.base_of_player(owner).unwrap().hp
}

/// A circular orbit `radii` base radii out around `owner`'s base.
fn orbit_around(sim: &Sim, owner: PlayerId, radii: f64) -> Particle {
    let (j, x, y) = base_at(sim, owner);
    let r = radii * sim.server.massive.radius[j];
    let v = (sim.server.rules.g * sim.server.massive.pull[j] / r).sqrt();
    Particle { x: x + r, y, vx: 0.0, vy: v }
}

fn buy(sim: &mut Sim, client: usize, ware: Ware) {
    sim.clients[client].session.buy(ware);
    sim.run_with(0.3, idle);
}

fn chat(s: &Session) -> Vec<String> {
    s.chat.iter().map(|c| c.text.clone()).collect()
}

fn agree(sim: &Sim) {
    for c in &sim.clients {
        assert_eq!(c.session.stats.hash_mismatches, 0);
        assert_eq!(c.session.stats.resyncs, 0);
    }
    sim.ships_agree().unwrap();
}

#[test]
fn a_shield_stops_one_shell() {
    let (mut sim, a, b, ann, bob) = duel(3);
    buy(&mut sim, a, Ware::Supply(Supply::Shield));
    assert_eq!(sim.server.shielded(ann), Some(true));
    assert!(sim.clients[b].session.world.as_ref().unwrap().players[&ann].shield);
    // Out in empty space, bob shoots at ann twice.
    place(&mut sim, ann, Particle { x: 2.0e11, y: 0.0, vx: 0.0, vy: 0.0 });
    place(&mut sim, bob, Particle { x: 2.0e11 + 1.0e9, y: 0.0, vx: 0.0, vy: 0.0 });
    sim.run_with(0.05, idle);
    let west = math::dir_to_angle(-1.0, 0.0);
    let t0 = sim.time;
    let shoot = |i: usize, t: f64| if i == b && (t < t0 + 0.1 || (t0 + 1.2..t0 + 1.3).contains(&t)) { Controls { fire: Some((west, 8000.0)), ..Controls::default() } } else { Controls::default() };
    sim.run_with(2.0, shoot);
    assert_eq!(sim.server.shielded(ann), Some(false));
    assert!(sim.server.ship(ann).is_some(), "the shield took the first shell");
    assert!(chat(&sim.clients[a].session).contains(&"ann's shield broke".to_string()));
    sim.run_with(2.0, shoot);
    assert!(sim.server.ship(ann).is_none(), "the second one gets through");
    agree(&sim);
}

#[test]
fn ammo_doubles_the_shells_and_the_rate_until_the_ship_is_lost() {
    let (mut sim, a, b, ann, _) = duel(2);
    let rules = sim.server.rules.clone();
    buy(&mut sim, a, Ware::Supply(Supply::Ammo));
    let mag = sim.server.magazine(ann).unwrap();
    assert_eq!((mag.max, mag.cooldown()), (2 * rules.shell_max, rules.shell_cooldown_ticks as u64 / 2));
    sim.run_with(0.5, idle);
    for i in [a, b] {
        assert_eq!(sim.clients[i].session.world.as_ref().unwrap().players[&ann].magazine, mag);
    }
    // Firing as fast as allowed, away from home (where shells are free anyway).
    place(&mut sim, ann, Particle { x: 2.0e11, y: 0.0, vx: 0.0, vy: 0.0 });
    let before = sim.server.shell_count();
    sim.run_with(3.0, |i, _| if i == a { Controls { fire: Some((0, 8000.0)), ..Controls::default() } } else { Controls::default() });
    let fired = sim.server.shell_count() - before;
    assert!((5..=7).contains(&fired), "{fired} shells in 3 s");
    sim.clients[a].session.send_chat("/respawn");
    sim.run_with(7.0, idle);
    assert_eq!(sim.server.magazine(ann).unwrap().max, rules.shell_max, "a new ship has the usual guns");
    assert_eq!(sim.clients[b].session.world.as_ref().unwrap().players[&ann].magazine.max, rules.shell_max);
    agree(&sim);
}

#[test]
fn repair_mends_part_of_the_base_and_armour_takes_less() {
    let (mut sim, a, _, ann, bob) = duel(4);
    sim.server.hurt_base(ann, bob, BASE_HP / 2);
    buy(&mut sim, a, Ware::Supply(Supply::Repair));
    assert_eq!(hp(&sim, ann), BASE_HP / 2 + REPAIR_HP);
    // Armour: a shell takes less off the base.
    buy(&mut sim, a, Ware::Upgrade(Upgrade::Armour));
    let before = hp(&sim, ann);
    let (_, x, y) = base_at(&sim, ann);
    place(&mut sim, bob, Particle { x: x + 2.0e8, y, vx: -20_000.0, vy: 0.0 });
    sim.run_with(4.0, idle);
    assert_eq!(before - hp(&sim, ann), DAMAGE.1 * 3 / 4, "a ram, through the armour");
    sim.run_with(0.5, idle);
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().base_of(ann).unwrap().hp, hp(&sim, ann));
}

#[test]
fn supplies_are_sold_at_home_and_carried_two_at_most() {
    let (mut sim, a, _, ann, _) = duel(5);
    for _ in 0..3 {
        buy(&mut sim, a, Ware::Supply(Supply::Warp));
    }
    assert_eq!(sim.server.items(ann).unwrap().warps, 2);
    assert!(chat(&sim.clients[a].session).contains(&"No more warp home for now".to_string()));
    place(&mut sim, ann, Particle { x: 2.0e11, y: 0.0, vx: 0.0, vy: 0.0 });
    buy(&mut sim, a, Ware::Supply(Supply::Well));
    assert_eq!(sim.server.items(ann).unwrap().wells, 0);
    assert!(chat(&sim.clients[a].session).contains(&"The shop is open only at home".to_string()));
    sim.run_with(0.5, idle);
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().players[&ann].items, sim.server.items(ann).unwrap());
}

#[test]
fn a_warp_jumps_home_and_leaves_the_cargo_behind() {
    let (mut sim, a, b, ann, _) = duel(6);
    buy(&mut sim, a, Ware::Supply(Supply::Warp));
    // Out among the scrap, take some.
    let (_, p) = *sim.server.pieces().iter().find(|p| p.0 == PieceKind::Scrap).unwrap();
    place(&mut sim, ann, Particle { x: p.x - 1.0e7, ..p });
    sim.run_with(0.5, idle);
    let carried = sim.server.purse(ann).unwrap().0.scrap;
    assert!(carried > 0);
    let pieces = sim.server.pieces().len();
    sim.clients[a].session.use_supply(Supply::Warp);
    sim.run_with(0.5, idle);
    let (j, x, y) = base_at(&sim, ann);
    let s = sim.server.ship(ann).unwrap().p;
    assert!((s.x - x).hypot(s.y - y) < HOME_RADII * sim.server.massive.radius[j], "home");
    assert_eq!(sim.server.purse(ann).unwrap().0.scrap, 0, "nothing came along");
    assert_eq!(sim.server.purse(ann).unwrap().1.scrap, 500 - Supply::Warp.price().scrap, "nor was it banked");
    assert_eq!(sim.server.pieces().len(), pieces + carried as usize);
    assert_eq!(sim.server.items(ann).unwrap().warps, 0);
    sim.run_with(1.0, idle);
    assert!(chat(&sim.clients[b].session).contains(&"ann jumped home".to_string()));
    agree(&sim);
}

#[test]
fn a_gravity_well_pulls_ships_in_for_a_while() {
    let (mut sim, a, b, _, bob) = duel(7);
    let ann = sim.player_id(a).unwrap();
    buy(&mut sim, a, Ware::Supply(Supply::Well));
    // Both still, 300 Mm apart, far from everything; ann drops a well.
    place(&mut sim, ann, Particle { x: 2.0e11, y: 0.0, vx: 0.0, vy: 0.0 });
    place(&mut sim, bob, Particle { x: 2.0e11 + 3.0e8, y: 0.0, vx: 0.0, vy: 0.0 });
    sim.run_with(0.2, idle);
    sim.clients[a].session.use_supply(Supply::Well);
    sim.run_with(3.0, idle);
    let v = sim.server.ship(bob).unwrap().p.vx;
    assert!(v < -100.0, "bob is pulled towards the well: {v} m/s");
    // Once it is gone (6 s on) nothing pulls there any more.
    sim.run_with(3.5, idle);
    place(&mut sim, bob, Particle { x: 2.0e11 + 3.0e8, y: 0.0, vx: 0.0, vy: 0.0 });
    sim.run_with(1.0, idle);
    let v = sim.server.ship(bob).unwrap().p.vx;
    assert!(v.abs() < 1.0, "{v} m/s after the well is gone");
    assert!(chat(&sim.clients[b].session).contains(&"ann dropped a gravity well".to_string()));
    agree(&sim);
}

#[test]
fn a_wider_zone_and_a_shipyard() {
    let (mut sim, a, _, ann, _) = duel(8);
    let rules = sim.server.rules.clone();
    // Outside the plain zone, inside the wider one: shells cost nothing only with it.
    let p = orbit_around(&sim, ann, 115.0);
    place(&mut sim, ann, p);
    sim.run_with(0.2, idle);
    assert!(!sim.clients[a].session.world.as_ref().unwrap().at_home(ann));
    buy(&mut sim, a, Ware::Upgrade(Upgrade::Zone));
    assert!(chat(&sim.clients[a].session).contains(&"The shop is open only at home".to_string()));
    go_home_and_buy(&mut sim, a, ann, Upgrade::Zone);
    let p = orbit_around(&sim, ann, 115.0);
    place(&mut sim, ann, p);
    sim.run_with(0.5, idle);
    assert!(sim.clients[a].session.world.as_ref().unwrap().at_home(ann));
    // A shipyard: the next ship comes sooner.
    go_home_and_buy(&mut sim, a, ann, Upgrade::Shipyard);
    sim.clients[a].session.send_chat("/respawn");
    sim.run_with(0.3 + rules.respawn_ticks as f64 * 0.7 / rules.tick_hz as f64, idle);
    assert!(sim.server.ship(ann).is_some(), "a ship within 3.5 s");
}

fn go_home_and_buy(sim: &mut Sim, client: usize, who: PlayerId, u: Upgrade) {
    let p = orbit_around(sim, who, 60.0);
    place(sim, who, p);
    sim.run_with(0.2, idle);
    buy(sim, client, Ware::Upgrade(u));
    assert!(sim.server.purse(who).unwrap().2.tier(u) >= 1);
}
