//! Scrap, in the bases mode. Clouds of it float around the center; a ship takes a piece by
//! passing close, carries it (heavier for every piece), and banks it on coming home, to spend
//! on upgrades. A ship that is lost spills what it carried, and loses a tier of every upgrade.

use super::*;
use gsim_core::MassiveView;
use gsim_proto::shop;

/// Pieces of scrap kept floating for every player in the round.
const SCRAP_PER_PLAYER: usize = 24;
/// They turn up in clouds of this many, one cloud every so often while there are too few.
const CLOUD: usize = 4;
const CLOUD_EVERY_SECONDS: f64 = 3.0;
/// Clouds turn up this far from the center, as a share of how far out the bases are.
const FIELD: (f64, f64) = (0.08, 0.55);
/// How far apart the pieces of a cloud start, and how fast they drift apart (m, m/s).
const CLOUD_SPREAD: f64 = 6.0e7;
const CLOUD_DRIFT: f64 = 15.0;
/// Crystals turn up this far from the center (as for scrap), first this long into a round and
/// then every so often (s).
const CRYSTAL_FIELD: (f64, f64) = (0.0, 0.15);
const CRYSTALS_FIRST_SECONDS: f64 = 60.0;
const CRYSTALS_EVERY_SECONDS: f64 = 90.0;
/// With crystal fever: first after this long, and then as often again.
const FEVER_EVERY_SECONDS: f64 = 30.0;
/// Supply crates turn up this far from the center (as for scrap), first this long into a round
/// and then every so often (s); one with nothing left to give gives this much scrap.
const CRATE_FIELD: (f64, f64) = (0.1, 0.7);
const CRATE_FIRST_SECONDS: f64 = 45.0;
const CRATE_EVERY_SECONDS: f64 = 120.0;
const CRATE_SCRAP: u32 = 5;
/// A piece further out than this many times the bases' ring, with no ship within
/// `STRAY_COMPANY` (m), fades away.
const STRAY_RINGS: f64 = 1.25;
const STRAY_COMPANY: f64 = 3.0e9;
/// How fast what a lost ship carried flies apart (m/s).
const SPILL_DRIFT: f64 = 40.0;
/// The most pieces a lost ship spills.
const SPILL_MAX: u32 = 40;

pub(super) struct Piece {
    pub id: PieceId,
    pub kind: PieceKind,
    pub p: Particle,
    /// Fading away since this tick (far out, with nobody near).
    pub fading: Option<Tick>,
}

/// One tick of every piece. Returns those gone: taken by a ship passing close (one of `ships`:
/// where each was at the start and at the end of the tick), crashed, or further than `far`
/// from the center.
pub(super) fn step_pieces(
    pieces: &mut [Piece],
    ships: &[Sweep],
    reach: &[f64],
    view: &MassiveView,
    rules: &GameRules,
    scratch: &mut Scratch,
    far: f64,
) -> Vec<(PieceId, Option<PlayerId>)> {
    let mut gone = Vec::new();
    for piece in pieces.iter_mut() {
        let before = (piece.p.x, piece.p.y);
        if step_particle(&mut piece.p, (0.0, 0.0), rules.shell_radius, view, rules, scratch).is_some() {
            gone.push((piece.id, None));
            continue;
        }
        let after = (piece.p.x, piece.p.y);
        if let Some((by, _)) = ships.iter().zip(reach).find(|(s, r)| swept_min_dist2(before, after, s.1, s.2) <= *r * *r) {
            gone.push((piece.id, Some(by.0)));
        } else if after.0 * after.0 + after.1 * after.1 > far * far {
            gone.push((piece.id, None));
        }
    }
    gone
}

impl Authority {
    /// Pieces gone this tick go, and what was taken goes into the takers' holds.
    pub(super) fn pick_up(&mut self, tick: Tick, gone: Vec<(PieceId, Option<PlayerId>)>) {
        let mut laden: Vec<PlayerId> = Vec::new();
        for (id, by) in gone {
            let Some(k) = self.pieces.iter().position(|p| p.id == id) else { continue };
            let piece = self.pieces.swap_remove(k);
            // A crate holds a supply that will be carried, one there is room for (or, failing that,
            // a few scrap).
            let pick = self.scrap_rng.u64() as usize;
            if let Some(p) = by.and_then(|by| self.players.get_mut(&by)) {
                if piece.kind == PieceKind::Crate {
                    let room: Vec<Supply> = Supply::CARRIED.into_iter().filter(|s| p.items.count(*s) < shop::CARRY).collect();
                    match room.get(pick % room.len().max(1)) {
                        Some(s) => *p.items.slot(*s).unwrap() += 1,
                        None => p.cargo.add(Purse { scrap: CRATE_SCRAP, crystals: 0 }),
                    }
                } else {
                    p.cargo.add(Purse::one(piece.kind));
                }
                if !laden.contains(&p.id) {
                    laden.push(p.id);
                }
            }
            self.event(Event::PieceGone { tick, id, by });
        }
        for id in laden {
            self.loadout(tick, id);
        }
    }

    /// The ship is lost (crashed into `body`, if it did): what it carried floats away, and what
    /// lasts as long as the ship is over.
    pub(super) fn spill(&mut self, tick: Tick, player: PlayerId, body: Option<u32>) {
        let Some(p) = self.players.get_mut(&player) else { return };
        // Rapid fire, a magnet and a target lock go with the ship.
        let changed = p.quick || p.lock || p.magnet_until > 0;
        (p.quick, p.magnet_until, p.lock) = (false, 0, false);
        if self.drop_cargo(tick, player, body) || changed {
            self.loadout(tick, player);
        }
    }

    /// What the ship carries floats away from where it is. False if it carried nothing.
    /// From a crash into `body`, the cargo goes gently round that body instead: the ship hit it
    /// far too fast for anything to be caught at that speed.
    fn drop_cargo(&mut self, tick: Tick, player: PlayerId, body: Option<u32>) -> bool {
        let Some(p) = self.players.get_mut(&player) else { return false };
        let (cargo, ship) = (std::mem::take(&mut p.cargo), p.ship);
        if let Some(ship) = ship {
            let kinds = std::iter::repeat_n(PieceKind::Crystal, cargo.crystals as usize).chain(std::iter::repeat_n(PieceKind::Scrap, cargo.scrap as usize));
            let spilled: Vec<(PieceKind, Particle)> = kinds
                .take(SPILL_MAX as usize)
                .map(|kind| (kind, match body.filter(|j| self.massive.alive[*j as usize]) {
                    Some(j) => self.around(j as usize, ship.p),
                    None => self.scatter(ship.p, 0.25 * CLOUD_SPREAD, SPILL_DRIFT),
                }))
                .collect();
            self.add_pieces(tick, spilled);
        }
        !cargo.is_empty()
    }

    /// On a slow circular orbit around body `j`, 6 to 10 radii out on the side where `from` (the
    /// wreck) is, going round the way it was.
    fn around(&mut self, j: usize, from: Particle) -> Particle {
        let m = &self.massive;
        let (bx, by, bvx, bvy, radius, pull) = (m.x[j], m.y[j], m.vx[j], m.vy[j], m.radius[j], m.pull[j].max(0.0));
        let (dx, dy, dvx, dvy) = (from.x - bx, from.y - by, from.vx - bvx, from.vy - bvy);
        let turn = if dx * dvy - dy * dvx < 0.0 { -1.0 } else { 1.0 };
        let a = dy.atan2(dx) + self.scrap_rng.range(-0.6, 0.6);
        let r = radius * self.scrap_rng.range(6.0, 10.0);
        let v = (self.rules.g * pull / r).sqrt();
        let (c, s) = (a.cos(), a.sin());
        Particle { x: bx + r * c, y: by + r * s, vx: bvx - turn * v * s, vy: bvy + turn * v * c }
    }

    /// Somewhere near `p`, drifting away from it.
    fn scatter(&mut self, p: Particle, spread: f64, drift: f64) -> Particle {
        let (a, b) = (self.scrap_rng.angle(), self.scrap_rng.angle());
        let (r, v) = (spread * self.scrap_rng.f64().sqrt(), drift * self.scrap_rng.f64());
        Particle { x: p.x + r * a.cos(), y: p.y + r * a.sin(), vx: p.vx + v * b.cos(), vy: p.vy + v * b.sin() }
    }

    fn add_pieces(&mut self, tick: Tick, new: Vec<(PieceKind, Particle)>) {
        let mut pieces = Vec::new();
        for (kind, p) in new {
            let id = self.next_piece_id;
            self.next_piece_id += 1;
            self.pieces.push(Piece { id, kind, p, fading: None });
            pieces.push(PieceInfo { id, kind, p, fading: None });
        }
        if !pieces.is_empty() {
            self.event(Event::Pieces { tick, pieces });
        }
    }

    /// A cloud of `n` pieces somewhere `field` (a share of how far out the bases are) from the
    /// center, clear of bodies and of the bases, going round with everything else. `None` if
    /// no clear spot turned up.
    fn cloud(&mut self, kind: PieceKind, n: usize, field: (f64, f64)) -> Option<Vec<(PieceKind, Particle)>> {
        let ring = self.spawn_r.1;
        let (cx, cy, cvx, cvy) = self.barycenter();
        for _ in 0..32 {
            let (a, r) = (self.scrap_rng.angle(), ring * self.scrap_rng.range(field.0, field.1));
            let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
            let m = &self.massive;
            let clear = (0..m.len()).filter(|&j| m.alive[j]).all(|j| {
                let keep_out = if m.fixed[j] { HOME_RADII } else { 20.0 } * m.radius[j] + CLOUD_SPREAD;
                (m.x[j] - x).powi(2) + (m.y[j] - y).powi(2) > keep_out * keep_out
            });
            if !clear {
                continue;
            }
            let center = self.circular_at(x, y, (cx, cy, cvx, cvy));
            return Some((0..n).map(|_| (kind, self.scatter(center, CLOUD_SPREAD, CLOUD_DRIFT))).collect());
        }
        None
    }

    /// Once a tick in a round of bases: more scrap while there is too little, crystals every
    /// so often, and whatever a ship brings home goes into the bank.
    pub(super) fn tend_scrap(&mut self, tick: Tick, playing: bool) {
        let hz = self.rules.tick_hz as f64;
        if playing && tick >= self.cloud_due {
            self.cloud_due = tick + (CLOUD_EVERY_SECONDS * hz) as Tick;
            if self.scrap_count() < self.scrap_wanted() {
                if let Some(cloud) = self.cloud(PieceKind::Scrap, CLOUD, FIELD) {
                    self.add_pieces(tick, cloud);
                }
            }
        }
        if playing && self.crate_due.is_some_and(|due| tick >= due) {
            if let Some(one) = self.cloud(PieceKind::Crate, 1, CRATE_FIELD) {
                self.add_pieces(tick, one);
            }
            self.crate_due = Some(tick + (CRATE_EVERY_SECONDS * hz) as Tick);
        }
        if playing && self.crystals_due.is_some_and(|due| tick >= due) {
            let fever = self.roll.twist == Twist::CrystalFever;
            let n = 1 + self.players.values().filter(|p| !p.spectating).count() / 6 + fever as usize;
            if let Some(cloud) = self.cloud(PieceKind::Crystal, n, CRYSTAL_FIELD) {
                self.add_pieces(tick, cloud);
            }
            let every = if fever { FEVER_EVERY_SECONDS } else { CRYSTALS_EVERY_SECONDS };
            self.crystals_due = Some(tick + (every * hz) as Tick);
            self.event(Event::CrystalsDue { due: self.crystals_due });
        }
        if tick.is_multiple_of(self.rules.tick_hz.max(1) as Tick) {
            self.fade_strays(tick);
        }
        let home: Vec<PlayerId> = self.players.values().filter(|p| !p.cargo.is_empty()).map(|p| p.id).filter(|id| self.in_zone(*id)).collect();
        for id in home {
            let cargo = std::mem::take(&mut self.players.get_mut(&id).unwrap().cargo);
            if let Some(b) = self.base_of_mut(id) {
                b.bank.add(cargo);
            }
            self.team_loadout(tick, id);
        }
    }

    /// Once a second: pieces that drifted far out with no ship near fade away, and are gone a
    /// few seconds later (unless somebody comes near meanwhile).
    fn fade_strays(&mut self, tick: Tick) {
        let (cx, cy, _, _) = self.barycenter();
        let far = STRAY_RINGS * self.spawn_r.1;
        let ships: Vec<(f64, f64)> = self.players.values().filter_map(|p| p.ship).map(|s| (s.p.x, s.p.y)).collect();
        let fade = (shop::FADE_SECONDS * self.rules.tick_hz as f64) as Tick;
        let (mut changes, mut gone) = (Vec::new(), Vec::new());
        for piece in &mut self.pieces {
            let (x, y) = (piece.p.x, piece.p.y);
            let stray = (x - cx).hypot(y - cy) > far && !ships.iter().any(|s| (s.0 - x).hypot(s.1 - y) < STRAY_COMPANY);
            match (stray, piece.fading) {
                (true, None) => {
                    piece.fading = Some(tick);
                    changes.push((piece.id, true));
                }
                (true, Some(since)) if tick >= since + fade => gone.push(piece.id),
                (false, Some(_)) => {
                    piece.fading = None;
                    changes.push((piece.id, false));
                }
                _ => {}
            }
        }
        for (id, fading) in changes {
            self.event(Event::PieceFade { tick, id, fading });
        }
        self.pieces.retain(|p| !gone.contains(&p.id));
        for id in gone {
            self.event(Event::PieceGone { tick, id, by: None });
        }
    }

    fn scrap_count(&self) -> usize {
        self.pieces.iter().filter(|p| p.kind == PieceKind::Scrap).count()
    }

    /// As much as the round rolled.
    fn scrap_wanted(&self) -> usize {
        let rush = if self.roll.twist == Twist::ScrapRush { 250 } else { 100 };
        SCRAP_PER_PLAYER * self.players.values().filter(|p| !p.spectating).count().max(2) * self.roll.scrap.max(1) as usize * rush / 10_000
    }

    /// A new round of bases starts with all its scrap, and crystals a while later.
    pub(super) fn sow_scrap(&mut self) {
        self.pieces.clear();
        self.crystals_due = None;
        self.crate_due = None;
        if self.mode != GameMode::Bases {
            return;
        }
        let tick = self.tick();
        for _ in 0..self.scrap_wanted().div_ceil(CLOUD) {
            let Some(cloud) = self.cloud(PieceKind::Scrap, CLOUD, FIELD) else { break };
            for (kind, p) in cloud {
                let id = self.next_piece_id;
                self.next_piece_id += 1;
                self.pieces.push(Piece { id, kind, p, fading: None });
            }
        }
        self.cloud_due = tick;
        let first = if self.roll.twist == Twist::CrystalFever { FEVER_EVERY_SECONDS } else { CRYSTALS_FIRST_SECONDS };
        self.crystals_due = Some(tick + (first * self.rules.tick_hz as f64) as Tick);
        self.crate_due = Some(tick + (CRATE_FIRST_SECONDS * self.rules.tick_hz as f64) as Tick);
    }

    /// The ship's engine and tank follow what it carries and what has been bought; everyone
    /// hears about it.
    pub(super) fn loadout(&mut self, tick: Tick, player: PlayerId) {
        let rules = self.rules.clone();
        let (bank, upgrades) = (self.bank_of(player), self.upgrades_of(player));
        let Some(p) = self.players.get_mut(&player) else { return };
        if let Some(ship) = p.ship.as_mut() {
            ship.refit(shop::kit(&rules, &p.upgrades, &p.cargo));
        }
        let loadout = Loadout { cargo: p.cargo, bank, upgrades, items: p.items, magazine: p.magazine, shield: p.shield, quick: p.quick, magnet_until: p.magnet_until, lock: p.lock };
        self.event(Event::Loadout { tick, player, loadout });
    }

    /// [`Self::loadout`] for everyone on the player's team (who share a bank and a base).
    fn team_loadout(&mut self, tick: Tick, player: PlayerId) {
        let team = self.players.get(&player).map_or(0, |p| p.team);
        let members: Vec<PlayerId> = self.players.values().filter(|p| p.id == player || (team != 0 && p.team == team)).map(|p| p.id).collect();
        for id in members {
            self.loadout(tick, id);
        }
    }

    /// What the player's team has banked.
    pub(super) fn bank_of(&self, player: PlayerId) -> Purse {
        self.base_of(player).map_or(Purse::default(), |b| b.bank)
    }

    /// The upgrades the player's team has bought (all shared), or the player's own without one.
    pub(super) fn upgrades_of(&self, player: PlayerId) -> Upgrades {
        let own = self.players.get(&player).map_or(Upgrades::default(), |p| p.upgrades);
        self.base_of(player).map_or(own, |b| b.upgrades)
    }

    /// Something from the shop, for what is banked: only at home (or waiting there for a ship).
    pub(super) fn buy(&mut self, conn: ConnId, ware: Ware) {
        let Some(&id) = self.by_conn.get(&conn) else { return };
        let (tick, rules) = (self.tick(), self.rules.clone());
        let home = self.in_zone(id) || self.players.get(&id).is_some_and(|p| p.ship.is_none() && p.respawn_tick.is_some() && self.base_alive(id));
        let hp = self.base_of(id).map_or(0, |b| b.hp);
        let (bank, upgrades) = (self.bank_of(id), self.upgrades_of(id));
        let sudden_death = self.sudden_death.is_some_and(|t| tick >= t);
        let Some(p) = self.players.get_mut(&id) else { return };
        let (name, price, full) = match ware {
            Ware::Upgrade(u) => {
                let tier = upgrades.tier(u) + 1;
                (u.name(), u.price(tier.min(shop::TIERS)), tier > shop::TIERS)
            }
            Ware::Supply(s) => {
                let full = match s {
                    Supply::Shield => p.shield,
                    Supply::Lock => p.lock,
                    Supply::RapidFire => p.quick,
                    Supply::Repair => hp >= BASE_HP || sudden_death,
                    _ => p.items.count(s) >= shop::CARRY,
                };
                (s.name(), s.price(), full)
            }
        };
        let refusal = if self.mode != GameMode::Bases || p.spectating {
            Some("Nothing to buy now".to_string())
        } else if !home {
            Some("The shop is open only at home".to_string())
        } else if full {
            Some(format!("No more {name} for now"))
        } else if !bank.covers(price) {
            Some(format!("The {name} costs {}", price.describe()))
        } else {
            None
        };
        if let Some(text) = refusal {
            return self.send(Target::One(conn), ServerMsg::Chat(ChatLine { kind: ChatKind::Error, from: None, text }));
        }
        match ware {
            Ware::Upgrade(_) => {}
            Ware::Supply(Supply::Shield) => p.shield = true,
            Ware::Supply(Supply::RapidFire) => p.quick = true,
            Ware::Supply(Supply::Lock) => p.lock = true,
            Ware::Supply(Supply::Repair) => {}
            Ware::Supply(s) => *p.items.slot(s).unwrap() += 1,
        }
        // What the team shares: the bank, the base and every upgrade.
        let b = self.base_of_mut(id).expect("a base, to be at home");
        b.bank.take(price);
        match ware {
            Ware::Upgrade(u) => b.upgrades.raise(u),
            Ware::Supply(Supply::Repair) => b.hp = (b.hp + REPAIR_HP).min(BASE_HP),
            _ => {}
        }
        let (team, ups) = (b.team, b.upgrades);
        self.bases_changed |= ware == Ware::Supply(Supply::Repair);
        // Everyone on the team flies with what was bought (a new magazine for more shells, and
        // rapid fire fills the buyer's).

        for p in self.players.values_mut().filter(|p| p.id == id || p.team == team) {
            p.upgrades = ups;
            let fresh = shop::magazine(tick, &rules, &ups, p.quick);
            p.magazine = if p.id == id && ware == Ware::Supply(Supply::RapidFire) { fresh } else { p.magazine.refit(tick, fresh, &rules) };
        }
        self.team_loadout(tick, id);
    }

    /// One of the supplies the ship carries: a jump home, or a gravity well where it is.
    pub(super) fn use_supply(&mut self, conn: ConnId, supply: Supply, angle: u16, speed: f32) {
        let Some(&id) = self.by_conn.get(&conn) else { return };
        let tick = self.tick();
        let home = self.base_spawn(id);
        let Some(p) = self.players.get_mut(&id).filter(|p| p.ship.is_some() && !p.spectating) else { return };
        let Some(count) = p.items.slot(supply).filter(|n| **n > 0) else { return };
        let ship = p.ship.unwrap();
        match supply {
            Supply::Warp => {
                let Some(at) = home else { return };
                *count -= 1;
                // The cargo stays behind.
                self.drop_cargo(tick, id, None);
                self.loadout(tick, id);
                let kit = self.players[&id].ship.map_or(ship.kit, |s| s.kit);
                self.place_ship(id, ShipState { p: at, kit, ..ship });
                self.event(Event::Warped { tick, player: id });
            }
            Supply::Well => {
                *count -= 1;
                // Off the ship as a shell would go.
                let fastest = shop::shell_speed_max(&self.rules, &p.upgrades);
                let speed = if speed.is_finite() { speed as f64 } else { 0.0 }.clamp(self.rules.shell_speed_min, fastest);
                let (dx, dy) = math::angle_to_dir(angle);
                let muzzle = self.rules.ship_radius + 2.0 * self.rules.shell_radius;
                let p = Particle { x: ship.p.x + dx * muzzle, y: ship.p.y + dy * muzzle, vx: ship.p.vx + dx * speed, vy: ship.p.vy + dy * speed };
                self.wells.push((tick, id, p));
                self.event(Event::Well { tick, player: id, p });
                self.loadout(tick, id);
            }
            Supply::Emp => {
                *count -= 1;
                // Every ship on another team near enough: its engines go dead for a while.
                let (team, hz) = (p.team, self.rules.tick_hz as f64);
                let until = tick + (shop::EMP_SECONDS * hz) as Tick;
                let near = |q: &Player| q.ship.is_some_and(|s| (s.p.x - ship.p.x).hypot(s.p.y - ship.p.y) <= shop::EMP_RADIUS);
                let hit: Vec<PlayerId> = self.players.values().filter(|q| q.id != id && (q.team != team || team == 0) && near(q)).map(|q| q.id).collect();
                for q in &hit {
                    self.players.get_mut(q).unwrap().stunned = (tick, until);
                }
                self.event(Event::Emp { tick, player: id, hit, until });
                self.loadout(tick, id);
            }
            Supply::Magnet => {
                *count -= 1;
                p.magnet_until = tick + (shop::MAGNET_SECONDS * self.rules.tick_hz as f64) as Tick;
                self.loadout(tick, id);
            }
            Supply::Afterburner => {
                *count -= 1;
                if let Some(s) = p.ship.as_mut() {
                    s.fuel += shop::AFTERBURNER_MMPS;
                }
                self.event(Event::Boost { tick, player: id, fuel: shop::AFTERBURNER_MMPS });
                self.loadout(tick, id);
            }
            _ => {}
        }
    }

    /// Test and operator hook: put scrap and crystals in a player's team's bank.
    pub fn grant(&mut self, player: PlayerId, purse: Purse) {
        let tick = self.tick();
        if let Some(b) = self.base_of_mut(player) {
            b.bank.add(purse);
            self.team_loadout(tick, player);
        }
    }

    /// What a player carries, has banked and has bought.
    pub fn purse(&self, player: PlayerId) -> Option<(Purse, Purse, Upgrades)> {
        self.players.get(&player).map(|p| (p.cargo, self.bank_of(player), self.upgrades_of(player)))
    }

    pub fn items(&self, player: PlayerId) -> Option<Items> {
        self.players.get(&player).map(|p| p.items)
    }

    /// Every piece floating free: (kind, where).
    pub fn pieces(&self) -> Vec<(PieceKind, Particle)> {
        self.pieces.iter().map(|p| (p.kind, p.p)).collect()
    }

    /// When crystals next turn up.
    pub fn crystals_due(&self) -> Option<Tick> {
        self.crystals_due
    }
}

impl Authority {
    /// Operator hook: supplies, as if bought (`count` of those carried, beyond what one may buy).
    pub fn grant_supply(&mut self, player: PlayerId, supply: Supply, count: u32) {
        let (tick, rules) = (self.tick(), self.rules.clone());
        let Some(p) = self.players.get_mut(&player) else { return };
        match supply {
            Supply::Lock => p.lock = true,
            Supply::Shield => p.shield = true,
            Supply::RapidFire => {
                p.quick = true;
                p.magazine = shop::magazine(tick, &rules, &p.upgrades, true);
            }
            Supply::Repair => {
                if let Some(b) = self.base_of_mut(player).filter(|b| b.hp > 0) {
                    b.hp = (b.hp + count * REPAIR_HP).min(BASE_HP);
                    self.bases_changed = true;
                }
            }
            _ => {
                let n = p.items.slot(supply).unwrap();
                *n = (*n as u32 + count).min(u8::MAX as u32) as u8;
            }
        }
        self.loadout(tick, player);
    }
}
