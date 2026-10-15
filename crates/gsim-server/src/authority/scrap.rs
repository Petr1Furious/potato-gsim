//! Scrap, in the bases mode. Clouds of it float around the centre; a ship takes a piece by
//! passing close, carries it (heavier for every piece), and banks it on coming home, to spend
//! on upgrades. A ship that is lost spills what it carried, and loses a tier of every upgrade.

use super::*;
use gsim_core::MassiveView;
use gsim_proto::shop::{self, PICKUP_RADIUS};

/// Pieces of scrap kept floating for every player in the round.
const SCRAP_PER_PLAYER: usize = 24;
/// They turn up in clouds of this many, one cloud every so often while there are too few.
const CLOUD: usize = 4;
const CLOUD_EVERY_SECONDS: f64 = 3.0;
/// Clouds turn up this far from the centre, as a share of how far out the bases are.
const FIELD: (f64, f64) = (0.08, 0.55);
/// How far apart the pieces of a cloud start, and how fast they drift apart (m, m/s).
const CLOUD_SPREAD: f64 = 6.0e7;
const CLOUD_DRIFT: f64 = 15.0;
/// Crystals turn up this far from the centre (as for scrap), first this long into a round and
/// then every so often (s).
const CRYSTAL_FIELD: (f64, f64) = (0.0, 0.15);
const CRYSTALS_FIRST_SECONDS: f64 = 60.0;
const CRYSTALS_EVERY_SECONDS: f64 = 90.0;
/// With crystal fever: first after this long, and then as often again.
const FEVER_EVERY_SECONDS: f64 = 30.0;
/// How fast what a lost ship carried flies apart (m/s).
const SPILL_DRIFT: f64 = 40.0;
/// The most pieces a lost ship spills.
const SPILL_MAX: u32 = 40;

pub(super) struct Piece {
    pub id: PieceId,
    pub kind: PieceKind,
    pub p: Particle,
}

/// One tick of every piece. Returns those gone: taken by a ship passing close (one of `ships`:
/// where each was at the start and at the end of the tick), crashed, or further than `far`
/// from the centre.
pub(super) fn step_pieces(
    pieces: &mut [Piece],
    ships: &[Sweep],
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
        if let Some(by) = ships.iter().find(|s| swept_min_dist2(before, after, s.1, s.2) <= PICKUP_RADIUS * PICKUP_RADIUS) {
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
            if let Some(p) = by.and_then(|by| self.players.get_mut(&by)) {
                p.cargo.add(Purse::one(piece.kind));
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

    /// The ship is lost: what it carried floats away, and every upgrade for it drops a tier.
    pub(super) fn spill(&mut self, tick: Tick, player: PlayerId) {
        let Some(p) = self.players.get_mut(&player) else { return };
        let had = p.upgrades;
        p.upgrades.lower_ship();
        p.quick = false;
        let changed = had != p.upgrades;
        if self.drop_cargo(tick, player) || changed {
            self.loadout(tick, player);
        }
    }

    /// What the ship carries floats away from where it is. False if it carried nothing.
    fn drop_cargo(&mut self, tick: Tick, player: PlayerId) -> bool {
        let Some(p) = self.players.get_mut(&player) else { return false };
        let (cargo, ship) = (std::mem::take(&mut p.cargo), p.ship);
        if let Some(ship) = ship {
            let kinds = std::iter::repeat_n(PieceKind::Crystal, cargo.crystals as usize).chain(std::iter::repeat_n(PieceKind::Scrap, cargo.scrap as usize));
            let spilled: Vec<(PieceKind, Particle)> = kinds
                .take(SPILL_MAX as usize)
                .map(|kind| (kind, self.scatter(ship.p, 0.25 * CLOUD_SPREAD, SPILL_DRIFT)))
                .collect();
            self.add_pieces(tick, spilled);
        }
        !cargo.is_empty()
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
            self.pieces.push(Piece { id, kind, p });
            pieces.push(PieceInfo { id, kind, p });
        }
        if !pieces.is_empty() {
            self.event(Event::Pieces { tick, pieces });
        }
    }

    /// A cloud of `n` pieces somewhere `field` (a share of how far out the bases are) from the
    /// centre, clear of bodies and of the bases, going round with everything else. `None` if
    /// no clear spot turned up.
    fn cloud(&mut self, kind: PieceKind, n: usize, field: (f64, f64)) -> Option<Vec<(PieceKind, Particle)>> {
        let ring = self.spawn_r.1;
        let (cx, cy, cvx, cvy) = self.barycentre();
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
            let centre = self.circular_at(x, y, (cx, cy, cvx, cvy));
            return Some((0..n).map(|_| (kind, self.scatter(centre, CLOUD_SPREAD, CLOUD_DRIFT))).collect());
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
        if playing && self.crystals_due.is_some_and(|due| tick >= due) {
            let fever = self.roll.twist == Twist::CrystalFever;
            let n = 2 + self.players.values().filter(|p| !p.spectating).count() / 3 + fever as usize;
            if let Some(cloud) = self.cloud(PieceKind::Crystal, n, CRYSTAL_FIELD) {
                self.add_pieces(tick, cloud);
            }
            let every = if fever { FEVER_EVERY_SECONDS } else { CRYSTALS_EVERY_SECONDS };
            self.crystals_due = Some(tick + (every * hz) as Tick);
            self.event(Event::CrystalsDue { due: self.crystals_due });
        }
        let home: Vec<PlayerId> = self.players.values().filter(|p| !p.cargo.is_empty()).map(|p| p.id).filter(|id| self.at_home(*id)).collect();
        for id in home {
            let cargo = std::mem::take(&mut self.players.get_mut(&id).unwrap().cargo);
            if let Some(b) = self.base_of_mut(id) {
                b.bank.add(cargo);
            }
            self.team_loadout(tick, id);
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
        if self.mode != GameMode::Bases {
            return;
        }
        let tick = self.tick();
        for _ in 0..self.scrap_wanted().div_ceil(CLOUD) {
            let Some(cloud) = self.cloud(PieceKind::Scrap, CLOUD, FIELD) else { break };
            for (kind, p) in cloud {
                let id = self.next_piece_id;
                self.next_piece_id += 1;
                self.pieces.push(Piece { id, kind, p });
            }
        }
        self.cloud_due = tick;
        let first = if self.roll.twist == Twist::CrystalFever { FEVER_EVERY_SECONDS } else { CRYSTALS_FIRST_SECONDS };
        self.crystals_due = Some(tick + (first * self.rules.tick_hz as f64) as Tick);
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
        let loadout = Loadout { cargo: p.cargo, bank, upgrades, items: p.items, magazine: p.magazine, shield: p.shield, quick: p.quick };
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

    /// The player's upgrades for their ship, and their team's for its base.
    pub(super) fn upgrades_of(&self, player: PlayerId) -> Upgrades {
        let ship = self.players.get(&player).map_or(Upgrades::default(), |p| p.upgrades);
        self.base_of(player).map_or(ship, |b| ship.with_base(&b.upgrades))
    }

    /// Something from the shop, for what is banked: only at home (or waiting there for a ship).
    pub(super) fn buy(&mut self, conn: ConnId, ware: Ware) {
        let Some(&id) = self.by_conn.get(&conn) else { return };
        let (tick, rules) = (self.tick(), self.rules.clone());
        let home = self.at_home(id) || self.players.get(&id).is_some_and(|p| p.ship.is_none() && p.respawn_tick.is_some() && self.base_alive(id));
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
            Ware::Upgrade(u) if !u.for_base() => p.upgrades.raise(u),
            Ware::Upgrade(_) => {}
            Ware::Supply(Supply::Shield) => p.shield = true,
            Ware::Supply(Supply::Ammo) => p.quick = true,
            Ware::Supply(Supply::Repair) => {}
            Ware::Supply(s) => *p.items.slot(s).unwrap() += 1,
        }
        // (A new magazine for more shells, and ammo fills it.)
        let fresh = shop::magazine(tick, &rules, &p.upgrades, p.quick);
        p.magazine = if ware == Ware::Supply(Supply::Ammo) { fresh } else { p.magazine.refit(tick, fresh, &rules) };
        // What the team shares: the bank, the base and what was bought for it.
        let b = self.base_of_mut(id).expect("a base, to be at home");
        b.bank.take(price);
        match ware {
            Ware::Upgrade(u) if u.for_base() => b.upgrades.raise(u),
            Ware::Supply(Supply::Repair) => {
                b.hp = (b.hp + REPAIR_HP).min(BASE_HP);
                self.bases_changed = true;
            }
            _ => {}
        }
        self.team_loadout(tick, id);
    }

    /// One of the supplies the ship carries: a jump home, or a gravity well where it is.
    pub(super) fn use_supply(&mut self, conn: ConnId, supply: Supply) {
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
                self.drop_cargo(tick, id);
                self.loadout(tick, id);
                let kit = self.players[&id].ship.map_or(ship.kit, |s| s.kit);
                self.place_ship(id, ShipState { p: at, kit, ..ship });
                self.event(Event::Warped { tick, player: id });
            }
            Supply::Well => {
                *count -= 1;
                self.wells.push((tick, ship.p.x, ship.p.y));
                self.event(Event::Well { tick, player: id, x: ship.p.x, y: ship.p.y });
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
