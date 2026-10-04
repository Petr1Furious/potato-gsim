use gsim_client_core::Controls;
use gsim_core::{Particle, ShipState};
use gsim_server::scenario::{build, RandomOpts};
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
    build("random", 11, &RandomOpts { count: 120, ..Default::default() }).unwrap()
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
        assert!(w.feed.iter().any(|f| f.1 == "ann destroyed bob"), "{:?}", w.feed);
    }
    sim.run_with(3.0, idle);
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
    let w = sim.clients[a].session.world.as_ref().unwrap();
    assert!(w.feed.iter().any(|f| f.1 == "ann crashed into Star"), "{:?}", w.feed);
}

#[test]
fn bad_clients_are_rejected() {
    let mut sim = Sim::new(quiet_scenario(), 9);
    sim.add_client("ann", Link::new(5.0, 0.0, 0.0));
    sim.add_client("ann", Link::new(5.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    assert!(sim.clients[0].session.joined());
    assert!(!sim.clients[1].session.joined());
    assert_eq!(sim.clients[1].session.rejected.as_deref(), Some("that name is already in use"));
}

#[test]
fn holding_an_orbit_captures_the_target() {
    let mut sim = Sim::new(quiet_scenario(), 10);
    sim.server.enable_objective();
    let a = sim.add_client("ann", Link::new(25.0, 5.0, 0.0));
    let b = sim.add_client("bob", Link::new(25.0, 5.0, 0.0));
    sim.run_with(1.0, idle);
    let (pa, pb) = (sim.player_id(a).unwrap(), sim.player_id(b).unwrap());
    assert_eq!(sim.server.target(), Some(0));
    assert_eq!(sim.clients[a].session.world.as_ref().unwrap().target, Some(0));
    // The spawn orbit is far outside the allowed band: nobody scores by doing nothing.
    sim.run_with(11.0, idle);
    assert_eq!(sim.server.captures(pa), Some(0));

    // A tight circular orbit around the star (period about half a second of real time).
    let rules = sim.server.rules.clone();
    let r = 2.0e9;
    let v = (rules.g * 2.0e30 / r).sqrt();
    sim.server.place_ship(pa, ShipState::new(Particle { x: r, y: 0.0, vx: 0.0, vy: v }, &rules));
    sim.run_with(5.0, idle);
    let held = sim.clients[b].session.world.as_ref().unwrap().holds.get(&pa).copied().unwrap_or(0);
    assert!(held > 200 && held < rules.hold_ticks, "progress is visible to others: {held}");
    assert_eq!(sim.server.captures(pa), Some(0));
    sim.run_with(6.0, idle);
    assert_eq!(sim.server.captures(pa), Some(1));
    assert_eq!(sim.server.captures(pb), Some(0));
    for c in &sim.clients {
        let w = c.session.world.as_ref().unwrap();
        assert_eq!(w.players[&pa].captures, 1);
        assert_eq!(w.players[&pa].score(&w.rules), rules.capture_points);
        assert!(w.feed.iter().any(|f| f.1 == "ann captured Star"), "{:?}", w.feed);
    }
    sim.ships_agree().unwrap();
}

#[test]
fn round_ends_and_a_new_world_starts() {
    let mut sim = Sim::new(small_world(), 11);
    let hz = sim.server.rules.tick_hz as u64;
    let opts = RandomOpts { count: 60, ..Default::default() };
    sim.server.set_rounds(5 * hz, hz, Some(("random".into(), opts)));
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
        assert!(w.target.is_some());
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
    assert_eq!(sim.server.target(), Some(2), "objective moved to the merged body, not elsewhere");
    let w = sim.clients[0].session.world.as_ref().unwrap();
    assert_eq!(w.target, Some(2));
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
        let target = sim.server.target().expect("a target is chosen while a ship is flying");
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
    assert_eq!(sim.server.target(), None, "no target until somebody is flying");

    let a = sim.add_client("ann", Link::new(20.0, 0.0, 0.0));
    sim.run_with(1.0, idle);
    assert_eq!(sim.server.round(), 1, "the first join starts round 1");
    let w = sim.clients[a].session.world.as_ref().unwrap();
    let left = w.round_end_tick.unwrap() - sim.server.tick();
    assert!(left > hz && left <= 3 * hz, "the clock started at the join, {left} ticks left");
    assert!(sim.server.target().is_some());

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
        sim.run_with(4.5, idle);
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
    assert_eq!(sim.server.target(), Some(2), "{:?}", sim.server.stats);
    assert_eq!(sim.server.stats.target_ineligible, 0);
}
