//! Bodies of interest move on when they stop being worth flying to.

use gsim_client_core::Controls;
use gsim_core::{Body, Particle, ShipState};
use gsim_server::authority::TargetPlan;
use gsim_server::scenario::{radius_from_mass, Scenario};
use gsim_testkit::*;

const LINK: Link = Link::new(15.0, 5.0, 0.0);

fn idle(_: usize, _: f64) -> Controls {
    Controls::default()
}

fn body(x: f64, vx: f64, mass: f64) -> Body {
    Body { x, y: 0.0, vx, vy: 0.0, mass, radius: radius_from_mass(mass) }
}

/// On a circular orbit `d` from a body of mass `around` at `x - d`.
fn moon(x: f64, mass: f64, around: f64, d: f64) -> Body {
    Body { vy: (6.67430e-11 * around / d).sqrt(), ..body(x, 0.0, mass) }
}

const HEAVY: u32 = 0;
const SWUNG: u32 = 1;
const RUNAWAY: u32 = 2;
const TWIN: u32 = 4;
const DASHER: u32 = 5;

/// A heavy planet with a light one right next to it, a planet on its way out of the world
/// (whose edge a speck far out marks), and a ship.
fn world() -> (Sim, usize) {
    let sc = Scenario {
        name: "interest".into(),
        bodies: vec![
            body(1.0e10, 0.0, 1.0e27),
            moon(1.0e10 + 1.0e8, 1.0e23, 1.0e27, 1.0e8),
            body(9.0e10, 2.0e5, 1.0e22),
            body(-1.0e11, 0.0, 1.0e20),
            // Circling the heavy one too, further out: the most like the moon.
            moon(1.3e10, 1.0e22, 1.0e27, 3.0e9),
            // Near the centre, but leaving fast.
            body(-2.0e10, -8.0e3, 1.0e22),
        ],
        names: Vec::new(),
        spawn_r: (3.0e10, 4.0e10),
    };
    let mut sim = Sim::new(sc, 1);
    sim.server.target_plan = TargetPlan::Fixed(1);
    let a = sim.add_client("ann", LINK);
    sim.run_with(1.0, idle);
    (sim, a)
}

#[test]
fn a_target_swung_around_by_a_heavier_body_moves_to_it() {
    let (mut sim, a) = world();
    sim.server.set_target(SWUNG);
    sim.run_with(1.5, idle);
    assert_eq!(sim.server.targets(), [HEAVY], "{:?}", sim.server.stats);
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().objectives.iter().map(|o| o.slot).collect::<Vec<_>>(), [HEAVY]);
}

#[test]
fn a_target_far_out_moves_on_unless_somebody_is_on_the_way() {
    let (mut sim, _) = world();
    let ann = sim.player_id(0).unwrap();
    // On the way: right behind it and gaining.
    let old = sim.server.ship(ann).unwrap();
    sim.server.place_ship(ann, ShipState { p: Particle { x: 5.0e10, y: 0.0, vx: 2.5e5, vy: 0.0 }, ..old });
    sim.server.set_target(RUNAWAY);
    sim.run_with(4.0, idle);
    assert!(sim.server.massive.x[RUNAWAY as usize] > 1.5e11, "it is far out by now");
    assert_eq!(sim.server.targets(), [RUNAWAY], "it waits for whoever is flying to it");
    // Turned away: it moves on.
    let old = sim.server.ship(ann).unwrap();
    sim.server.place_ship(ann, ShipState { p: Particle { vx: -1.0e4, ..old.p }, ..old });
    sim.run_with(1.5, idle);
    let now = sim.server.targets();
    assert!(now == [HEAVY] || now == [TWIN], "{now:?} {:?}", sim.server.stats);
}

#[test]
fn operators_reelect_by_name_or_kind() {
    let (mut sim, a) = world();
    sim.server.state.op("ann");
    sim.server.set_target(RUNAWAY);
    sim.clients[a].session.send_chat("/reelect @t");
    sim.run_with(0.5, idle);
    let now = sim.server.targets();
    assert!(now == [HEAVY] || now == [TWIN], "{now:?}");
    let said: Vec<String> = sim.clients[a].session.chat.iter().map(|c| c.text.clone()).collect();
    assert!(said.iter().any(|t| *t == format!("Moved B2 to B{}", now[0])), "{said:?}");
}

#[test]
fn when_the_heavier_body_is_taken_the_most_alike_one_takes_over() {
    let (mut sim, _) = world();
    sim.server.target_plan = TargetPlan::Fixed(2);
    sim.server.set_target(HEAVY);
    sim.server.set_target(SWUNG);
    sim.run_with(1.5, idle);
    let mut targets = sim.server.targets();
    targets.sort();
    assert_eq!(targets, [HEAVY, TWIN], "{:?}", sim.server.stats);
}

#[test]
fn a_target_racing_away_moves_on_before_it_is_far() {
    let (mut sim, _) = world();
    sim.server.set_target(DASHER);
    sim.run_with(1.5, idle);
    let m = &sim.server.massive;
    assert!(m.x[DASHER as usize].abs() < 1.0e11, "still well within the world");
    assert!(!sim.server.targets().contains(&DASHER), "{:?}", sim.server.targets());
}
