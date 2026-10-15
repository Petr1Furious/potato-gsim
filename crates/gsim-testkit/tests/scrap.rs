//! Scrap in the bases mode: picking it up, its weight, banking it at home, the shop, and
//! what a lost ship spills.

use gsim_client_core::{Controls, Session};
use gsim_core::{Body, Kit, Particle, ShipState};
use gsim_proto::shop::{self, Upgrade};
use gsim_proto::{Roll, Twist, GameMode, PieceKind, PlayerId, Purse, Ware};
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
    sim.server.fix_roll(Roll { scrap: 100, mines: 0, twist: Twist::Plain, teams: 1 });
    sim.server.set_mode(GameMode::Bases);
    let a = sim.add_client("ann", LINK);
    let b = sim.add_client("bob", LINK);
    sim.run_with(1.5, idle);
    let (ann, bob) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    (sim, a, b, ann, bob)
}

fn place(sim: &mut Sim, who: PlayerId, p: Particle) {
    let old = sim.server.ship(who).unwrap();
    sim.server.place_ship(who, ShipState { p, ..old });
}

/// Fly `who` along with a piece of scrap, a little behind it, so that the ship takes it.
fn take_scrap(sim: &mut Sim, who: PlayerId) {
    let before = sim.server.purse(who).unwrap().0.scrap;
    let (_, p) = *sim.server.pieces().iter().find(|p| p.0 == PieceKind::Scrap).expect("some scrap");
    place(sim, who, Particle { x: p.x - 1.0e7, ..p });
    sim.run_with(0.5, idle);
    assert!(sim.server.purse(who).unwrap().0.scrap > before, "took nothing");
}

/// Back on a circular orbit around one's own base.
fn go_home(sim: &mut Sim, who: PlayerId) {
    let j = sim.server.base_of_player(who).unwrap().slot as usize;
    let m = &sim.server.massive;
    let r = 50.0 * m.radius[j];
    let v = (sim.server.rules.g * m.pull[j] / r).sqrt();
    let p = Particle { x: m.x[j] + r, y: m.y[j], vx: 0.0, vy: v };
    place(sim, who, p);
}

fn chat(s: &Session) -> Vec<String> {
    s.chat.iter().map(|c| c.text.clone()).collect()
}

/// Every client sees every piece where the server has it, bit for bit, and agrees about
/// every ship (its engine and tank included).
fn agree(sim: &Sim) {
    let tick = sim.server.tick();
    let mut server: Vec<(PieceKind, Particle)> = sim.server.pieces();
    server.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
    for c in &sim.clients {
        let w = c.session.world.as_ref().unwrap();
        let mut local: Vec<(PieceKind, Particle)> = w.pieces.values().filter_map(|p| p.track.at(tick).map(|at| (p.kind, *at))).collect();
        local.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
        assert_eq!(local, server);
        assert_eq!(c.session.stats.hash_mismatches, 0);
    }
    sim.ships_agree().unwrap();
}

#[test]
fn scrap_floats_around_the_center_and_everyone_sees_the_same() {
    let (mut sim, _, _, _, _) = duel(1);
    let pieces = sim.server.pieces();
    assert!(pieces.len() >= 40, "{} pieces", pieces.len());
    for (kind, p) in &pieces {
        let r = (p.x * p.x + p.y * p.y).sqrt();
        assert_eq!(*kind, PieceKind::Scrap);
        assert!(r < 0.7 * 3.0e10, "{r} m out");
    }
    sim.run_with(5.0, idle);
    agree(&sim);
}

#[test]
fn a_piece_taken_weighs_the_ship_down_until_it_is_banked_at_home() {
    let (mut sim, a, b, ann, _) = duel(2);
    let rules = sim.server.rules.clone();
    take_scrap(&mut sim, ann);
    let (cargo, bank, _) = sim.server.purse(ann).unwrap();
    assert!(cargo.scrap >= 1 && bank.scrap == 0);
    let kit = sim.server.ship(ann).unwrap().kit;
    assert_eq!(kit, shop::kit(&rules, &Default::default(), &cargo));
    assert!(kit.push < kit.burn);
    sim.run_with(1.0, idle);
    for i in [a, b] {
        assert_eq!(sim.clients[i].session.world.as_ref().unwrap().players[&ann].cargo, cargo);
    }
    agree(&sim);

    // Home: into the bank, and the ship is light again.
    go_home(&mut sim, ann);
    sim.run_with(1.0, idle);
    let (now, bank, _) = sim.server.purse(ann).unwrap();
    assert!(now.is_empty() && bank == cargo, "{now:?} {bank:?}");
    assert_eq!(sim.server.ship(ann).unwrap().kit, shop::kit(&rules, &Default::default(), &Purse::default()));
    let w = sim.clients[a].session.world.as_ref().unwrap();
    assert!(w.alerts.iter().any(|al| al.text.starts_with("Banked")), "{:?}", w.alerts);
    agree(&sim);
}

#[test]
fn a_laden_ship_gains_less_from_the_same_burn() {
    let (mut sim, a, _, ann, _) = duel(3);
    let speed = |sim: &Sim| {
        let s = sim.server.ship(ann).unwrap().p;
        (s.vx * s.vx + s.vy * s.vy).sqrt()
    };
    let burn = |sim: &mut Sim| {
        // Far from everything: straight out, a second at full thrust.
        place(sim, ann, Particle { x: 0.0, y: 2.0e11, vx: 0.0, vy: 0.0 });
        sim.run_with(1.0, |i, _| if i == a { Controls { angle: 0, thrust: 100, ..Controls::default() } } else { Controls::default() });
        sim.run_with(0.5, idle);
        speed(sim)
    };
    let light = burn(&mut sim);
    sim.server.state.op("ann");
    // Ten pieces of scrap: half again as heavy.
    for _ in 0..10 {
        take_scrap(&mut sim, ann);
        if sim.server.purse(ann).unwrap().0.scrap >= 10 {
            break;
        }
    }
    let carried = sim.server.purse(ann).unwrap().0.scrap;
    let laden = burn(&mut sim);
    let expect = 100.0 / (100.0 + 5.0 * carried as f64);
    assert!((laden / light - expect).abs() < 0.03, "{laden} vs {light} with {carried} scrap");
    agree(&sim);
}

#[test]
fn the_shop_sells_the_next_tier_at_home_only() {
    let (mut sim, a, b, ann, bob) = duel(4);
    let rules = sim.server.rules.clone();
    sim.server.grant(ann, Purse { scrap: 12, crystals: 0 });
    // Away from home: no.
    place(&mut sim, ann, Particle { x: 0.0, y: 2.0e11, vx: 0.0, vy: 0.0 });
    sim.clients[a].session.buy(Ware::Upgrade(Upgrade::Thrust));
    sim.run_with(0.5, idle);
    assert!(chat(&sim.clients[a].session).contains(&"The shop is open only at home".to_string()));
    assert_eq!(sim.server.purse(ann).unwrap().2.tier(Upgrade::Thrust), 0);
    // At home: yes, while the bank lasts.
    go_home(&mut sim, ann);
    sim.run_with(0.5, idle);
    let fuel = sim.server.ship(ann).unwrap().fuel;
    for u in [Upgrade::Thrust, Upgrade::Thrust, Upgrade::DeltaV] {
        sim.clients[a].session.buy(Ware::Upgrade(u));
    }
    sim.run_with(0.5, idle);
    let (_, bank, ups) = sim.server.purse(ann).unwrap();
    assert_eq!((ups.tier(Upgrade::Thrust), ups.tier(Upgrade::DeltaV)), (2, 0), "the tank was too dear");
    assert_eq!(bank.scrap, 0);
    let ship = sim.server.ship(ann).unwrap();
    assert_eq!(ship.kit.burn, Kit::of(&rules).burn * 2);
    // A bigger tank comes filled.
    sim.server.grant(ann, Purse { scrap: 4, crystals: 0 });
    sim.clients[a].session.buy(Ware::Upgrade(Upgrade::DeltaV));
    sim.run_with(0.5, idle);
    let ship = sim.server.ship(ann).unwrap();
    assert_eq!(ship.kit.tank, rules.fuel_max_mmps * 3 / 2);
    assert!(ship.fuel >= fuel + rules.fuel_max_mmps / 2 - 1000, "{} from {fuel}", ship.fuel);
    // More shells, and faster ones.
    sim.server.grant(ann, Purse { scrap: 8, crystals: 0 });
    sim.clients[a].session.buy(Ware::Upgrade(Upgrade::Shells));
    sim.clients[a].session.buy(Ware::Upgrade(Upgrade::ShellSpeed));
    sim.run_with(0.5, idle);
    assert_eq!(sim.server.magazine(ann).unwrap().max, rules.shell_max * 2);
    for i in [a, b] {
        let w = sim.clients[i].session.world.as_ref().unwrap();
        assert_eq!(w.players[&ann].upgrades, sim.server.purse(ann).unwrap().2);
        assert_eq!(w.players[&ann].magazine, sim.server.magazine(ann).unwrap());
    }
    assert!(sim.clients[a].session.world.as_ref().unwrap().shell_speed_max() > rules.shell_speed_max);
    assert_eq!(sim.server.purse(bob).unwrap().2, Default::default());
    agree(&sim);
}

#[test]
fn a_lost_ship_spills_its_cargo_but_keeps_its_upgrades() {
    let (mut sim, a, b, ann, _) = duel(5);
    let rules = sim.server.rules.clone();
    sim.server.grant(ann, Purse { scrap: 12, crystals: 0 });
    go_home(&mut sim, ann);
    sim.run_with(0.5, idle);
    sim.clients[a].session.buy(Ware::Upgrade(Upgrade::Thrust));
    sim.clients[a].session.buy(Ware::Upgrade(Upgrade::Thrust));
    sim.run_with(0.5, idle);
    take_scrap(&mut sim, ann);
    take_scrap(&mut sim, ann);
    let carried = sim.server.purse(ann).unwrap().0.scrap;
    let before = sim.server.pieces().len();
    let at = sim.server.ship(ann).unwrap().p;
    sim.clients[a].session.send_chat("/kill ann");
    sim.run_with(0.5, idle);
    let pieces = sim.server.pieces();
    assert_eq!(pieces.len(), before + carried as usize);
    let near = pieces.iter().filter(|(_, p)| (p.x - at.x).hypot(p.y - at.y) < 5.0e8).count();
    assert!(near >= carried as usize, "{near} of {carried} near the wreck");
    let (cargo, _, ups) = sim.server.purse(ann).unwrap();
    assert!(cargo.is_empty());
    assert_eq!(ups.tier(Upgrade::Thrust), 2);
    sim.run_with(6.0, idle);
    let ship = sim.server.ship(ann).expect("a new ship");
    assert_eq!(ship.kit, shop::kit(&rules, &ups, &Purse::default()));
    let w = sim.clients[b].session.world.as_ref().unwrap();
    assert_eq!(w.players[&ann].upgrades.tier(Upgrade::Thrust), 2);
    assert!(sim.clients[a].session.world.as_ref().unwrap().alerts.iter().any(|al| al.text.starts_with("Lost")));
    agree(&sim);
}

#[test]
fn a_crash_leaves_the_cargo_going_gently_round_what_was_hit() {
    let (mut sim, _, _, ann, bob) = duel(6);
    take_scrap(&mut sim, ann);
    take_scrap(&mut sim, ann);
    let carried = sim.server.purse(ann).unwrap().0.scrap as usize;
    let before = sim.server.pieces().len();
    // Straight into bob's base at 20 km/s.
    let j = sim.server.base_of_player(bob).unwrap().slot as usize;
    let (bx, by, radius) = (sim.server.massive.x[j], sim.server.massive.y[j], sim.server.massive.radius[j]);
    place(&mut sim, ann, Particle { x: bx + 2.0e8, y: by, vx: -20_000.0, vy: 0.0 });
    sim.run_with(1.0, idle);
    assert!(sim.server.ship(ann).is_none(), "crashed");
    let spilled: Vec<Particle> = sim.server.pieces().into_iter().skip(before).map(|p| p.1).collect();
    assert_eq!(spilled.len(), carried);
    for p in &spilled {
        let (d, v) = ((p.x - bx).hypot(p.y - by), p.vx.hypot(p.vy));
        assert!(d < 11.0 * radius && v < 3_500.0, "{d} m out at {v} m/s");
    }
    // Going round, not falling in.
    sim.run_with(3.0, idle);
    assert_eq!(sim.server.pieces().len(), before + carried);
    agree(&sim);
}

#[test]
fn pieces_far_out_fade_away_unless_somebody_is_near() {
    let (mut sim, a, b, ann, bob) = duel(7);
    take_scrap(&mut sim, ann);
    take_scrap(&mut sim, ann);
    let carried = sim.server.purse(ann).unwrap().0.scrap as usize;
    // Spilled far beyond the bases (their ring is 30 Gm out), with bob keeping it company.
    let far = Particle { x: 6.0e10, y: 0.0, vx: 0.0, vy: 0.0 };
    place(&mut sim, ann, far);
    place(&mut sim, bob, Particle { x: far.x + 1.0e9, ..far });
    sim.run_with(0.2, idle);
    sim.clients[a].session.send_chat("/kill ann");
    let count = |sim: &Sim| sim.server.pieces().iter().filter(|(_, p)| p.x > 5.0e10).count();
    sim.run_with(10.0, idle);
    assert_eq!(count(&sim), carried, "bob is near: they stay");
    // bob goes home: they fade, and are gone some 6 s later.
    sim.clients[a].session.send_chat("/kill bob");
    sim.run_with(2.0, idle);
    let w = sim.clients[b].session.world.as_ref().unwrap();
    assert!(w.pieces.values().filter(|p| p.fading.is_some()).count() >= carried, "they fade");
    assert_eq!(count(&sim), carried);
    sim.run_with(6.0, idle);
    assert_eq!(count(&sim), 0, "and are gone");
    agree(&sim);
}
