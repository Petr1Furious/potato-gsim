//! Crystal mines in the bases mode: where they open, the crystals they give, and their
//! comeback; and crystals turning up at the centre.

use gsim_client_core::{Controls, Session};
use gsim_core::objective::orbit_status;
use gsim_core::{Body, Particle, ShipState};
use gsim_proto::{Roll, Twist, GameMode, PieceKind, PlayerId, MINE_SECONDS, MINE_STOCK};
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
    sim.server.fix_roll(Roll { scrap: 100, mines: 0, twist: Twist::Plain });
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

/// Put `who` on a circular orbit 20 radii above the first mine. Returns its body.
fn circle_mine(sim: &mut Sim, who: PlayerId) -> u32 {
    let slot = sim.server.mines().first().expect("a mine").slot;
    let m = &sim.server.massive;
    let j = slot as usize;
    let r = 20.0 * m.radius[j];
    let v = (sim.server.rules.g * m.pull[j] / r).sqrt();
    let p = Particle { x: m.x[j] + r, y: m.y[j], vx: m.vx[j], vy: m.vy[j] + v };
    place(sim, who, p);
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
fn mines_open_on_planets() {
    let (sim, a, b, _, _) = duel(1);
    let mines = sim.server.mines();
    assert_eq!(mines.len(), 2, "one, and one more for two players");
    for s in &mines {
        let j = s.slot as usize;
        assert!(sim.server.massive.alive[j] && !sim.server.massive.fixed[j] && sim.server.massive.mass[j] > 0.0);
        assert_eq!(s.left, MINE_STOCK);
    }
    assert_ne!(mines[0].slot, mines[1].slot);
    for i in [a, b] {
        assert_eq!(sim.clients[i].session.world.as_ref().unwrap().mines, mines);
    }
}

#[test]
fn a_mine_gives_crystals_while_held_until_it_runs_out() {
    let (mut sim, a, b, ann, _) = duel(2);
    let slot = circle_mine(&mut sim, ann);
    sim.run_with(MINE_SECONDS + 0.5, idle);
    assert_eq!(sim.server.purse(ann).unwrap().0.crystals, 1);
    assert_eq!(sim.server.mines().iter().find(|m| m.slot == slot).unwrap().left, MINE_STOCK - 1);
    // A crystal weighs the ship down as scrap does, only more.
    let kit = sim.server.ship(ann).unwrap().kit;
    assert!(kit.push < kit.burn);
    sim.run_with(2.0 * MINE_SECONDS, idle);
    assert_eq!(sim.server.purse(ann).unwrap().0.crystals, MINE_STOCK);
    assert!(!sim.server.mines().iter().any(|m| m.slot == slot), "spent, it moves on");
    sim.run_with(1.0, idle);
    for i in [a, b] {
        let w = sim.clients[i].session.world.as_ref().unwrap();
        assert_eq!(w.players[&ann].cargo.crystals, MINE_STOCK);
        assert_eq!(w.mines, sim.server.mines());
    }
    let w = sim.clients[a].session.world.as_ref().unwrap();
    assert!(w.alerts.iter().any(|al| al.text == "+1 crystal"));
    // Another opens some 20 s later.
    sim.run_with(20.0, idle);
    assert_eq!(sim.server.mines().len(), 2);
    in_sync(&sim);
    sim.ships_agree().unwrap();
}

#[test]
fn more_players_more_mines() {
    let mut sim = Sim::new(planets(), 5);
    sim.server.set_rounds(0, 600, None);
    sim.server.fix_roll(Roll { scrap: 100, mines: 0, twist: Twist::Plain });
    sim.server.set_mode(GameMode::Bases);
    for name in ["ann", "bob", "cat", "dan"] {
        sim.add_client(name, LINK);
        sim.run_with(1.0, idle);
    }
    sim.run_with(1.0, idle);
    // cat and dan joined a round under way: they watch it.
    assert_eq!(sim.server.bases().len(), 2);
    assert_eq!(sim.server.mines().len(), 2);
    // In the next round all four play.
    sim.server.fix_roll(Roll { scrap: 100, mines: 0, twist: Twist::Plain });
    sim.server.set_mode(GameMode::Bases);
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.bases().len(), 4);
    assert_eq!(sim.server.mines().len(), 3);
}

#[test]
fn crystals_turn_up_at_the_centre_every_so_often() {
    let (mut sim, a, _, _, _) = duel(6);
    let hz = sim.server.rules.tick_hz as f64;
    let due = sim.server.crystals_due().expect("crystals are on their way");
    let wait = (due - sim.server.tick()) as f64 / hz;
    assert!((55.0..=60.0).contains(&wait), "first in {wait} s");
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().crystals_due, Some(due));
    sim.run_with(wait + 0.5, idle);
    let crystals: Vec<Particle> = sim.server.pieces().into_iter().filter(|p| p.0 == PieceKind::Crystal).map(|p| p.1).collect();
    assert!(crystals.len() >= 2, "{} crystals", crystals.len());
    for c in &crystals {
        assert!(c.x.hypot(c.y) < 0.2 * 3.0e10, "{c:?} is not near the centre");
    }
    let next = sim.server.crystals_due().unwrap();
    assert!(((next - sim.server.tick()) as f64 / hz - 89.5).abs() < 1.0);
    let w = sim.clients[a].session.world.as_ref().unwrap();
    assert_eq!(w.crystals_due, Some(next));
    assert!(chat(&sim.clients[a].session).contains(&"Crystals turned up near the centre".to_string()));
    in_sync(&sim);
}

#[test]
fn orbits_around_mines_keep_their_shape() {
    // In a generated world, where the bases would drag a ship off a planet they do not pull
    // the same way, if they reached that far.
    for seed in [7, 8] {
        let sc = gsim_server::scenario::build("random", seed, &Default::default()).unwrap();
        let mut sim = Sim::new(sc, seed);
        sim.server.set_rounds(0, 600, None);
        sim.server.fix_roll(Roll { scrap: 100, mines: 0, twist: Twist::Plain });
        sim.server.set_mode(GameMode::Bases);
        let a = sim.add_client("ann", LINK);
        sim.add_client("bob", LINK);
        sim.run_with(1.0, idle);
        let ann = sim.player_id(a).unwrap();
        let slot = circle_mine(&mut sim, ann) as usize;
        sim.run_with(12.0, idle);
        let m = &sim.server.massive;
        assert!(m.alive[slot], "{seed}: the planet itself is gone");
        let s = sim.server.ship(ann).expect("still circling").p;
        let body = Particle { x: m.x[slot], y: m.y[slot], vx: m.vx[slot], vy: m.vy[slot] };
        let o = orbit_status(&s, &body, m.pull[slot], m.radius[slot], &sim.server.rules);
        assert!(o.bound && o.ecc < 0.03, "{seed}: {o:?}");
    }
}
