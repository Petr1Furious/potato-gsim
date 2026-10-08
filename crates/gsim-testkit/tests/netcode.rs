use gsim_client_core::Controls;
use gsim_core::{Particle, ShipState};
use gsim_server::scenario::{build, Params};
use gsim_testkit::*;

/// Busy pilots: thrust on and off, heading sweeping, different per client.
fn flying(i: usize, t: f64) -> Controls {
    let phase = t * (1.3 + i as f64 * 0.7);
    Controls {
        angle: ((phase * 9000.0) as i64).rem_euclid(65536) as u16,
        thrust: if (phase as u64) % 3 == 0 { 0 } else { 60 + ((phase * 7.0) as u64 % 41) as u8 },
        fire: None,
    }
}

fn idle(_: usize, _: f64) -> Controls {
    Controls::default()
}

fn small_world() -> gsim_server::Scenario {
    build("random", 11, &Params::from([("count".to_string(), 120.0)])).unwrap()
}

#[test]
fn clean_link_never_mispredicts() {
    let mut sim = Sim::new(small_world(), 1);
    sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.add_client("bob", Link::new(35.0, 0.0, 0.0));
    sim.run_with(2.0, idle);
    sim.run_with(15.0, flying);
    sim.run_with(1.0, idle);
    sim.ships_agree().unwrap();
    assert!(sim.server.stats.cmds > 200, "{:?}", sim.server.stats);
    assert_eq!(sim.server.stats.late_cmds, 0);
    for c in &sim.clients {
        let s = c.session.stats;
        assert!(s.hash_checks >= 10, "{s:?}");
        assert_eq!(s.hash_mismatches, 0, "{s:?}");
        assert_eq!(s.retimed_cmds, 0, "{s:?}");
        // The input delay covers this latency: remote inputs arrive before they apply,
        // so nothing ever has to be rolled back or corrected.
        assert_eq!(s.late_remote_inputs, 0, "{s:?}");
        assert_eq!(s.ship_corrections, 0, "{s:?}");
        assert!(s.ship_checks > 20, "{s:?}");
    }
}

#[test]
fn lossy_jittery_link_converges_exactly() {
    let mut sim = Sim::new(small_world(), 2);
    sim.add_client("ann", Link::new(80.0, 60.0, 0.10));
    sim.add_client("bob", Link::new(40.0, 30.0, 0.05));
    sim.add_client("cat", Link::new(5.0, 0.0, 0.0));
    sim.run_with(2.0, idle);
    sim.run_with(20.0, flying);
    sim.run_with(2.0, idle);
    sim.ships_agree().unwrap();
    for c in &sim.clients {
        let s = c.session.stats;
        assert_eq!(s.hash_mismatches, 0, "{s:?}");
        assert!(s.hash_checks >= 10, "{s:?}");
    }
    // The laggy client's inputs reach the others in their past: handled by per-ship rollback.
    assert!(sim.clients[2].session.stats.rollbacks > 0);
}

#[test]
fn very_high_latency_adapts_its_input_lead() {
    let mut sim = Sim::new(small_world(), 3);
    sim.add_client("far", Link::new(250.0, 10.0, 0.0));
    sim.add_client("near", Link::new(10.0, 0.0, 0.0));
    sim.run_with(3.0, idle);
    let late_before = sim.server.stats.late_cmds;
    sim.run_with(12.0, flying);
    sim.run_with(2.0, idle);
    sim.ships_agree().unwrap();
    assert!(sim.clients[0].session.input_lead_ticks() >= 15);
    // Once the clock is synced, even a 500 ms round trip does not make inputs late.
    assert_eq!(sim.server.stats.late_cmds, late_before, "{:?}", sim.server.stats);
    assert_eq!(sim.clients[0].session.stats.retimed_cmds, 0);
}

#[test]
fn late_joiner_and_leaver() {
    let mut sim = Sim::new(small_world(), 4);
    sim.add_client("ann", Link::new(30.0, 5.0, 0.0));
    sim.run_with(4.0, flying);
    sim.add_client("bob", Link::new(60.0, 20.0, 0.02));
    sim.run_with(8.0, flying);
    sim.run_with(1.5, idle);
    sim.ships_agree().unwrap();
    assert_eq!(sim.clients[1].session.world.as_ref().unwrap().players.len(), 2);
    sim.disconnect(0);
    sim.run_with(1.0, idle);
    assert_eq!(sim.clients[1].session.world.as_ref().unwrap().players.len(), 1);
    assert_eq!(sim.server.player_count(), 1);
}

#[test]
fn forced_divergence_is_detected_and_resynced() {
    let mut sim = Sim::new(small_world(), 5);
    sim.add_client("ann", Link::new(30.0, 0.0, 0.0));
    sim.run_with(3.0, flying);
    assert_eq!(sim.clients[0].session.stats.hash_mismatches, 0);
    // Move a body by one metre on the server only.
    sim.server.massive.nudge(3, 1.0, 0.0);
    sim.run_with(4.0, flying);
    let s = sim.clients[0].session.stats;
    assert_eq!(s.hash_mismatches, 1, "{s:?}");
    assert_eq!(s.resyncs, 1, "{s:?}");
    let checks = s.hash_checks;
    sim.run_with(4.0, flying);
    sim.run_with(1.0, idle);
    let s = sim.clients[0].session.stats;
    assert!(s.hash_checks > checks && s.hash_mismatches == 1, "{s:?}");
    sim.ships_agree().unwrap();
}

#[test]
fn shell_destroys_target_and_victim_respawns() {
    let mut sim = Sim::new(quiet_scenario(), 6);
    let a = sim.add_client("ann", Link::new(25.0, 5.0, 0.0));
    let b = sim.add_client("bob", Link::new(25.0, 5.0, 0.0));
    sim.run_with(1.0, idle);
    let (pa, pb) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    let rules = sim.server.rules.clone();
    let at = |x: f64| ShipState::new(Particle { x, y: 1.5e11, vx: 0.0, vy: 0.0 }, &rules);
    // Far enough that the shell is armed before it arrives.
    sim.server.place_ship(pa, at(0.0));
    sim.server.place_ship(pb, at(4.0e8));
    sim.run_with(0.5, idle);
    sim.run_with(0.3, |i, _| Controls { angle: 0, thrust: 0, fire: (i == 0).then_some((0, 8000.0)) });
    sim.run_with(1.5, idle);
    assert_eq!(sim.server.score(pa), Some((1, 0)));
    assert_eq!(sim.server.score(pb), Some((0, 1)));
    assert!(sim.server.ship(pb).is_none(), "victim is waiting to respawn");
    for c in &sim.clients {
        let w = c.session.world.as_ref().unwrap();
        assert_eq!(w.players[&pa].kills, 1);
        assert_eq!(w.players[&pb].deaths, 1);
        assert!(w.players[&pb].ship.is_none());
        assert!(w.players[&pb].respawn_tick.is_some());
        assert!(c.session.chat.iter().any(|l| l.text == "ann destroyed bob"), "{:?}", c.session.chat);
    }
    sim.run_with(5.0, idle);
    assert!(sim.server.ship(pb).is_some(), "victim respawned");
    assert_eq!(sim.server.shell_count(), 0);
    sim.ships_agree().unwrap();
}

#[test]
fn shell_cannot_explode_before_it_is_armed() {
    let mut sim = Sim::new(quiet_scenario(), 7);
    let a = sim.add_client("ann", Link::new(10.0, 0.0, 0.0));
    let b = sim.add_client("bob", Link::new(10.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    let (pa, pb) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    let rules = sim.server.rules.clone();
    let at = |x: f64| ShipState::new(Particle { x, y: 1.5e11, vx: 0.0, vy: 0.0 }, &rules);
    sim.server.place_ship(pa, at(0.0));
    sim.server.place_ship(pb, at(1.5e7)); // inside the blast radius of a shell leaving the muzzle
    sim.run_with(0.5, idle);
    sim.run_with(0.3, |i, _| Controls { angle: 0, thrust: 0, fire: (i == 0).then_some((0, 8000.0)) });
    sim.run_with(2.0, idle);
    assert_eq!(sim.server.score(pb), Some((0, 0)), "point-blank shots fly past before arming");
    assert_eq!(sim.server.score(pa), Some((0, 0)));
}

#[test]
fn crashing_into_a_body_is_fatal() {
    let mut sim = Sim::new(quiet_scenario(), 8);
    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    let pa = sim.player_id(a).unwrap();
    let rules = sim.server.rules.clone();
    // Aimed straight at the star at 200 km/s.
    sim.server.place_ship(pa, ShipState::new(Particle { x: 6.0e9, y: 0.0, vx: -2.0e5, vy: 0.0 }, &rules));
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.score(pa), Some((0, 1)));
    assert!(sim.clients[a].session.chat.iter().any(|l| l.text == "ann crashed into Star"), "{:?}", sim.clients[a].session.chat);
}


#[test]
fn holding_an_orbit_captures_the_target() {
    let mut sim = Sim::new(quiet_scenario(), 10);
    sim.server.enable_objective();
    let a = sim.add_client("ann", Link::new(25.0, 5.0, 0.0));
    let b = sim.add_client("bob", Link::new(25.0, 5.0, 0.0));
    sim.run_with(1.0, idle);
    let (pa, pb) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    assert_eq!(sim.server.targets(), [0]);
    assert!(sim.clients[a].session.world.as_ref().unwrap().is_target(0));
    // The spawn orbit is far outside the allowed band: nobody scores by doing nothing.
    sim.run_with(11.0, idle);
    assert_eq!(sim.server.captures(pa), Some(0));

    // A tight circular orbit around the star (period about half a second of real time).
    let rules = sim.server.rules.clone();
    let r = 2.0e9;
    let v = (rules.g * 2.0e30 / r).sqrt();
    sim.server.place_ship(pa, ShipState::new(Particle { x: r, y: 0.0, vx: 0.0, vy: v }, &rules));
    sim.run_with(5.0, idle);
    let (slot, held) = sim.clients[b].session.world.as_ref().unwrap().holds.get(&pa).copied().unwrap_or((9, 0));
    assert!(slot == 0 && held > 200 && held < rules.hold_ticks, "progress is visible to others: {held} on {slot}");
    assert_eq!(sim.server.captures(pa), Some(0));
    sim.run_with(6.0, idle);
    assert_eq!(sim.server.captures(pa), Some(1));
    assert_eq!(sim.server.captures(pb), Some(0));
    for c in &sim.clients {
        let w = c.session.world.as_ref().unwrap();
        assert_eq!(w.players[&pa].captures, 1);
        assert_eq!(w.players[&pa].score(&w.rules), rules.capture_points as i64);
        assert!(c.session.chat.iter().any(|l| l.text == "ann captured Star"), "{:?}", c.session.chat);
    }
    sim.ships_agree().unwrap();
}

#[test]
fn round_ends_and_a_new_world_starts() {
    let mut sim = Sim::new(small_world(), 11);
    let hz = sim.server.rules.tick_hz as u64;
    let params = Params::from([("count".to_string(), 60.0)]);
    sim.server.set_rounds(5 * hz, hz, Some(("random".into(), params)));
    // Escaped-body removal is part of the shared simulation: hashes must still agree.
    sim.server.rules.escape_radius = 2.0e11;
    sim.server.enable_objective();
    sim.add_client("ann", Link::new(30.0, 10.0, 0.0));
    sim.add_client("bob", Link::new(60.0, 20.0, 0.02));
    sim.run_with(4.0, flying);
    for c in &sim.clients {
        let w = c.session.world.as_ref().unwrap();
        assert_eq!((w.round, w.next_round_tick), (1, None));
        assert!(w.round_end_tick.is_some());
    }
    sim.run_with(1.5, flying);
    assert_eq!(sim.server.round(), 1);
    assert!(sim.clients[0].session.world.as_ref().unwrap().next_round_tick.is_some(), "intermission announced");
    sim.run_with(4.0, flying);
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.round(), 2);
    assert_eq!(sim.server.massive.len(), 60);
    for c in &sim.clients {
        let w = c.session.world.as_ref().unwrap();
        assert_eq!(w.round, 2);
        assert_eq!(w.next_round_tick, None);
        assert_eq!(w.eph.get(w.head - 1).unwrap().x.len(), 60, "client switched to the new world");
        assert!(!w.objectives.is_empty());
        let s = c.session.stats;
        // A new round is not a resync, and the new world is hash-checked like the old one.
        assert_eq!((s.hash_mismatches, s.resyncs), (0, 0), "{s:?}");
        assert!(s.hash_checks >= 8, "{s:?}");
    }
    sim.ships_agree().unwrap();
}

#[test]
fn objective_follows_a_target_that_merges() {
    use gsim_core::Body;
    let body = |x: f64, mass: f64, radius: f64| Body { x, y: 0.0, vx: 0.0, vy: 0.0, mass, radius };
    let mut sc = quiet_scenario();
    // Two heavy bodies about to fall into each other, and decoys elsewhere.
    sc.bodies.push(body(1.0e11, 4.0e25, 1.2e7));
    sc.bodies.push(body(1.0e11 + 5.0e7, 5.0e25, 1.3e7));
    for i in 0..6 {
        sc.bodies.push(Body { x: 0.0, y: 3.0e11 + 1.0e10 * i as f64, vx: 2.0e4, vy: 0.0, mass: 2.0e25, radius: 9.6e6 });
    }
    let mut sim = Sim::new(sc, 12);
    sim.server.set_target(1); // the lighter of the pair
    sim.add_client("ann", Link::new(25.0, 5.0, 0.0));
    sim.run_with(1.5, idle);
    assert!(!sim.server.massive.alive[1], "the lighter body was absorbed");
    assert!(sim.server.massive.alive[2]);
    assert_eq!(sim.server.targets(), [2], "objective moved to the merged body, not elsewhere");
    let w = sim.clients[0].session.world.as_ref().unwrap();
    assert!(w.is_target(2) && w.objectives.len() == 1);
}

#[test]
fn targets_are_picked_near_the_players() {
    use gsim_core::Body;
    let mut sc = quiet_scenario();
    // Twelve equally heavy bodies on a ring; slot k+1 sits at angle k * 30 degrees.
    for k in 0..12 {
        let a = k as f64 * std::f64::consts::TAU / 12.0;
        sc.bodies.push(Body { x: 2.0e11 * a.cos(), y: 2.0e11 * a.sin(), vx: 0.0, vy: 0.0, mass: 3.0e25, radius: 1.0e7 });
    }
    for seed in 0..8 {
        let mut sim = Sim::new(sc.clone(), 20 + seed);
        let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
        sim.run_with(1.0, idle);
        let pa = sim.player_id(a).unwrap();
        let rules = sim.server.rules.clone();
        // Park the only ship next to slot 4 (angle 90 degrees).
        sim.server.place_ship(pa, ShipState::new(Particle { x: 0.0, y: 2.05e11, vx: 0.0, vy: 0.0 }, &rules));
        sim.server.enable_objective();
        sim.run_with(0.1, idle);
        let target = *sim.server.targets().first().expect("a target is chosen while a ship is flying");
        assert!((3..=5).contains(&target), "target {target} should be one of the three bodies nearest the ship");
    }
}

#[test]
fn rounds_wait_for_players() {
    let mut sim = Sim::new(small_world(), 30);
    let hz = sim.server.rules.tick_hz as u64;
    sim.server.set_rounds(3 * hz, hz, None);
    sim.server.enable_objective();
    // An empty server does not burn through rounds.
    sim.run_with(10.0, idle);
    assert_eq!(sim.server.round(), 0);
    assert!(sim.server.targets().is_empty(), "no target until somebody is flying");

    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.round(), 1, "the first join starts round 1");
    let w = sim.clients[a].session.world.as_ref().unwrap();
    let left = w.round_end_tick.unwrap() - sim.server.tick();
    assert!(left > hz && left <= 3 * hz, "the clock started at the join, {left} ticks left");
    assert_eq!(sim.server.targets().len(), 1);

    // Everyone leaves: the round runs out, and no new one starts.
    sim.disconnect(a);
    sim.run_with(12.0, idle);
    assert_eq!(sim.server.round(), 1);

    let b = sim.add_client("bob", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.round(), 2, "the next join gets a fresh round");
    let w = sim.clients[b].session.world.as_ref().unwrap();
    assert_eq!((w.round, w.next_round_tick), (2, None));
    assert_eq!(sim.clients[b].session.stats.hash_mismatches, 0);
}

#[test]
fn ships_spawn_near_the_target_but_not_on_top_of_it() {
    use gsim_core::Body;
    let mut sc = quiet_scenario();
    // The objective: a planet on the far side of the star from the usual spawn annulus.
    sc.bodies.push(Body { x: -3.0e11, y: 0.0, vx: 0.0, vy: 0.0, mass: 3.0e25, radius: 1.0e7 });
    let mut sim = Sim::new(sc, 40);
    sim.server.set_target(1);
    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    let rules = sim.server.rules.clone();
    let band = rules.orbit_max_apo_radii * 1.0e7;
    for life in 0..6 {
        sim.run_with(1.0, idle);
        let pa = sim.player_id(a).unwrap();
        let ship = sim.server.ship(pa).expect("alive").p;
        let (tx, ty) = (sim.server.massive.x[1], sim.server.massive.y[1]);
        let d = ((ship.x - tx).powi(2) + (ship.y - ty).powi(2)).sqrt();
        // Spawned about a second ago and drifting slowly: still essentially the spawn distance.
        assert!(d > 3.5 * band, "life {life}: {d:e} m is too close to the scoring band ({band:e} m)");
        assert!(d < 0.6e11, "life {life}: {d:e} m is nowhere near the target");
        // Die (crash into the star) and come back.
        sim.server.place_ship(pa, ShipState::new(Particle { x: 6.0e9, y: 0.0, vx: -2.0e5, vy: 0.0 }, &rules));
        sim.run_with(6.5, idle);
    }
    sim.ships_agree().unwrap();
}

#[test]
fn target_survives_a_fast_swing_past_a_neighbour() {
    use gsim_core::Body;
    let mut sc = quiet_scenario();
    // A light body in a tight, fast orbit around a heavy one, far from the star: it moves
    // faster than the system's escape speed at that distance, half the time outwards.
    let (m_heavy, d): (f64, f64) = (5.0e27, 4.0e8);
    let v = (6.67430e-11 * m_heavy / d).sqrt();
    sc.bodies.push(Body { x: 4.0e11, y: 0.0, vx: 0.0, vy: 0.0, mass: m_heavy, radius: 4.0e7 });
    sc.bodies.push(Body { x: 4.0e11 + d, y: 0.0, vx: 0.0, vy: v, mass: 1.0e24, radius: 3.0e6 });
    let mut sim = Sim::new(sc, 50);
    sim.server.rules.escape_radius = 4.0e12;
    assert!(v > (2.0 * 6.67430e-11 * 2.0e30 / 4.0e11f64).sqrt(), "the moon really is that fast: {v}");
    sim.server.set_target(2);
    sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(12.0, idle);
    assert_eq!(sim.server.targets(), [2], "{:?}", sim.server.stats);
    assert_eq!(sim.server.stats.target_ineligible, 0);
}

fn firing(_: usize, _: f64) -> Controls {
    Controls { angle: 0, thrust: 0, fire: Some((0, 8000.0)) }
}

/// The shells a client believes it has, at the server's tick.
fn shells_seen(sim: &Sim, client: usize) -> u32 {
    let w = sim.clients[client].session.world.as_ref().unwrap();
    w.me().unwrap().magazine.at(sim.server.tick(), &w.rules)
}

#[test]
fn shells_run_out_and_come_back() {
    let mut sim = Sim::new(quiet_scenario(), 60);
    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    let pa = sim.player_id(a).unwrap();
    let max = sim.server.rules.shell_max;
    assert_eq!((max, sim.server.shells_left(pa)), (10, Some(10)));

    // Holding the trigger: still one a second, until there are none.
    sim.run_with(4.5, firing);
    assert_eq!(sim.server.shells_left(pa), Some(5));
    assert_eq!(shells_seen(&sim, a), 5);
    sim.run_with(5.0, firing);
    assert_eq!(sim.server.shells_left(pa), Some(0));
    // The last one left about 0.4 s ago. Nothing for four seconds, then one every two.
    sim.run_with(5.0, idle);
    assert_eq!(sim.server.shells_left(pa), Some(0));
    sim.run_with(1.5, idle);
    assert_eq!(sim.server.shells_left(pa), Some(1));
    assert_eq!(shells_seen(&sim, a), 1);
    sim.run_with(4.0, idle);
    assert_eq!(sim.server.shells_left(pa), Some(3));
    sim.run_with(20.0, idle);
    assert_eq!(sim.server.shells_left(pa), Some(max), "never more than the full stock");

    // Every shot makes the rest wait again.
    sim.run_with(0.3, firing);
    assert_eq!(sim.server.shells_left(pa), Some(max - 1));
    sim.run_with(5.0, idle);
    assert_eq!(sim.server.shells_left(pa), Some(max - 1));
    sim.run_with(1.5, idle);
    assert_eq!(sim.server.shells_left(pa), Some(max));
    assert_eq!(shells_seen(&sim, a), max);
}

#[test]
fn an_empty_ship_fires_as_soon_as_a_shell_is_back() {
    let mut sim = Sim::new(quiet_scenario(), 61);
    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    let pa = sim.player_id(a).unwrap();
    // Ten in the first ten seconds, then one every six (the wait and one shell's worth).
    sim.run_with(30.0, firing);
    let fired = sim.server.stats.shells_fired;
    assert!((12..=14).contains(&fired), "{fired} shells in 30 s of holding the trigger");
    assert_eq!(sim.server.shells_left(pa), Some(0));
}

#[test]
fn a_new_ship_has_all_its_shells() {
    let mut sim = Sim::new(quiet_scenario(), 62);
    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    let pa = sim.player_id(a).unwrap();
    let rules = sim.server.rules.clone();
    sim.run_with(4.5, firing);
    assert_eq!(sim.server.shells_left(pa), Some(5));
    // Straight into the star, and back a few seconds later.
    sim.server.place_ship(pa, ShipState::new(Particle { x: 6.0e9, y: 0.0, vx: -2.0e5, vy: 0.0 }, &rules));
    sim.run_with(1.0, firing);
    assert_eq!(sim.server.score(pa), Some((0, 1)));
    sim.run_with(5.5, idle);
    assert!(sim.server.ship(pa).is_some(), "respawned");
    assert_eq!(sim.server.shells_left(pa), Some(rules.shell_max));
    assert_eq!(shells_seen(&sim, a), rules.shell_max);
}

/// A star with a ring of twelve equal planets; slot k+1 sits at angle k * 30 degrees.
fn ring_world() -> gsim_server::Scenario {
    let mut sc = quiet_scenario();
    for k in 0..12 {
        let a = k as f64 * std::f64::consts::TAU / 12.0;
        sc.bodies.push(gsim_core::Body { x: 2.0e11 * a.cos(), y: 2.0e11 * a.sin(), vx: 0.0, vy: 0.0, mass: 3.0e25, radius: 1.0e7 });
    }
    sc
}

/// Put a ship on a small circular orbit around a body.
fn park_around(sim: &mut Sim, player: gsim_proto::PlayerId, slot: usize) {
    let (m, rules) = (&sim.server.massive, sim.server.rules.clone());
    let r = 5.0 * m.radius[slot];
    let v = (rules.g * m.mass[slot] / r).sqrt();
    let p = Particle { x: m.x[slot] + r, y: m.y[slot], vx: m.vx[slot], vy: m.vy[slot] + v };
    sim.server.place_ship(player, ShipState::new(p, &rules));
}

#[test]
fn a_target_moves_on_when_its_time_is_up_unless_somebody_orbits_it() {
    let mut sim = Sim::new(ring_world(), 70);
    let hz = sim.server.rules.tick_hz;
    sim.server.rules.target_ticks = 5 * hz;
    // Long enough that nothing is captured here.
    sim.server.rules.hold_ticks = 60 * hz;
    sim.server.enable_objective();
    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(2.0, idle);
    let pa = sim.player_id(a).unwrap();
    let first = sim.server.targets()[0];
    let left = sim.server.target_left(first).unwrap();
    assert!(left > 3 * hz && left < 5 * hz, "the time is running: {left} ticks left");
    let seen = |sim: &Sim| sim.clients[a].session.world.as_ref().unwrap().objectives.clone();
    assert!(seen(&sim)[0].slot == first && seen(&sim)[0].left.abs_diff(left) <= 10, "{:?}", seen(&sim));

    sim.run_with(4.0, idle);
    let second = sim.server.targets()[0];
    assert_ne!(second, first, "another body takes over");
    assert_eq!(sim.server.stats.target_expired, 1);
    assert_eq!(seen(&sim)[0].slot, second);
    let said = format!("New target: {}", sim.clients[a].session.world.as_ref().unwrap().body_name(second));
    assert!(sim.clients[a].session.chat.iter().any(|l| l.text == said), "{:?}", sim.clients[a].session.chat);

    // On an orbit around it: the time stands still for as long as that lasts.
    park_around(&mut sim, pa, second as usize);
    sim.run_with(0.5, idle);
    let left = sim.server.target_left(second).unwrap();
    sim.run_with(8.0, idle);
    assert_eq!(sim.server.targets(), [second]);
    assert_eq!(sim.server.target_left(second), Some(left));
    assert_eq!(seen(&sim)[0].left, left);
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().holds.get(&pa).map(|h| h.0), Some(second));

    // Away again: it runs out like any other.
    let rules = sim.server.rules.clone();
    sim.server.place_ship(pa, ShipState::new(Particle { x: 0.0, y: 3.0e11, vx: 0.0, vy: 0.0 }, &rules));
    sim.run_with(5.5, idle);
    assert_ne!(sim.server.targets(), [second]);
    assert_eq!(sim.server.stats.target_expired, 2);
    assert_eq!(sim.server.stats.captures, 0);
    assert_eq!(sim.clients[a].session.stats.hash_mismatches, 0);
}

#[test]
fn the_number_of_targets_follows_the_players_or_is_fixed() {
    use gsim_server::authority::TargetPlan;
    let mut sim = Sim::new(ring_world(), 80);
    // Time does not get in the way here.
    sim.server.rules.target_ticks = 0;
    sim.server.enable_objective();
    let check = |sim: &Sim, want: usize, why: &str| {
        let mut slots = sim.server.targets();
        assert_eq!(slots.len(), want, "{why}");
        for c in sim.clients.iter().filter(|c| c.connected) {
            let seen: Vec<u32> = c.session.world.as_ref().unwrap().objectives.iter().map(|o| o.slot).collect();
            assert_eq!(seen, slots, "{why}");
        }
        slots.sort();
        slots.dedup();
        assert_eq!(slots.len(), want, "all different: {why}");
    };
    // One for every two players by default, and never none.
    let link = Link::new(20.0, 0.0, 0.0);
    let ann = sim.add_client("ann", link.clone());
    sim.run_with(1.0, idle);
    check(&sim, 1, "one player");
    let names = ["bob", "cat", "dan", "eve"];
    let others: Vec<usize> = names.iter().map(|n| sim.add_client(n, link.clone())).collect();
    sim.run_with(1.0, idle);
    check(&sim, 2, "five players");
    sim.disconnect(others[3]);
    sim.disconnect(others[2]);
    sim.run_with(0.5, idle);
    check(&sim, 1, "three players");

    sim.server.target_plan = TargetPlan::PerPlayers(1);
    sim.run_with(0.5, idle);
    check(&sim, 3, "one each");
    sim.server.target_plan = TargetPlan::Fixed(6);
    sim.run_with(0.5, idle);
    check(&sim, 6, "six whoever is there");
    sim.disconnect(ann);
    sim.run_with(0.5, idle);
    check(&sim, 6, "six whoever is there");
    sim.server.target_plan = TargetPlan::Fixed(2);
    sim.run_with(0.5, idle);
    check(&sim, 2, "two");
}

#[test]
fn the_two_ways_of_counting_targets_do_not_mix() {
    use gsim_server::authority::TargetPlan;
    use gsim_server::net::{build_authority, ServerOptions};
    let opts = |per: Option<u32>, total: Option<u32>| ServerOptions { players_per_target: per, targets: total, ..ServerOptions::default() };
    assert_eq!(build_authority(&opts(None, None)).unwrap().target_plan, TargetPlan::PerPlayers(2));
    assert_eq!(build_authority(&opts(Some(3), None)).unwrap().target_plan, TargetPlan::PerPlayers(3));
    assert_eq!(build_authority(&opts(None, Some(4))).unwrap().target_plan, TargetPlan::Fixed(4));
    assert!(build_authority(&opts(Some(3), Some(4))).is_err());
}

#[test]
fn deaths_cost_a_point_and_scores_can_go_below_zero() {
    let rules = gsim_core::GameRules::new(86400.0, 60);
    assert_eq!(rules.score(1, 0, 0), 2);
    assert_eq!(rules.score(0, 0, 1), 5);
    assert_eq!(rules.score(3, 4, 2), 3 * 2 + 2 * 5 - 4);
    assert_eq!(rules.respawn_ticks, 5 * 60);

    let mut sim = Sim::new(quiet_scenario(), 90);
    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    let pa = sim.player_id(a).unwrap();
    sim.clients[a].session.send_chat("/respawn");
    sim.run_with(4.5, idle);
    assert!(sim.server.ship(pa).is_none(), "not back before five seconds");
    let w = sim.clients[a].session.world.as_ref().unwrap();
    assert_eq!(w.players[&pa].score(&w.rules), -1);
    sim.run_with(1.0, idle);
    assert!(sim.server.ship(pa).is_some());
}

#[test]
fn everyone_sees_a_ships_colour_and_its_changes() {
    let mut sim = Sim::new(quiet_scenario(), 91);
    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    let b = sim.add_client("bob", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    let pa = sim.player_id(a).unwrap();
    let seen = |sim: &Sim, c: usize| sim.clients[c].session.world.as_ref().unwrap().players[&pa].color;
    // What the join announced, to those already there and to those who came later.
    assert_eq!((seen(&sim, a), seen(&sim, b)), ([255, 255, 255], [255, 255, 255]));
    sim.clients[a].session.set_color([10, 200, 30]);
    sim.run_with(0.5, idle);
    assert_eq!((seen(&sim, a), seen(&sim, b)), ([10, 200, 30], [10, 200, 30]));
    let c = sim.add_client("cat", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    assert_eq!(seen(&sim, c), [10, 200, 30]);
}
