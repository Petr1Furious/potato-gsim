//! What players gather, carry home and spend in the bases mode, and what it buys. Shared, so
//! that both sides work out a ship's engine and guns the same way.

use gsim_core::{GameRules, Kit, Magazine, Tick};
use serde::{Deserialize, Serialize};

/// Something to pick up, floating free: flying close enough takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PieceKind {
    Scrap,
    Crystal,
}

/// How close a ship has to pass a piece to take it (m).
pub const PICKUP_RADIUS: f64 = 8.0e7;

/// Scrap and crystals: carried in a ship's hold, or banked at home.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Purse {
    pub scrap: u32,
    pub crystals: u32,
}

impl Purse {
    pub fn is_empty(&self) -> bool {
        self.scrap == 0 && self.crystals == 0
    }

    pub fn add(&mut self, other: Purse) {
        self.scrap += other.scrap;
        self.crystals += other.crystals;
    }

    pub fn covers(&self, price: Purse) -> bool {
        self.scrap >= price.scrap && self.crystals >= price.crystals
    }

    pub fn take(&mut self, price: Purse) {
        self.scrap -= price.scrap;
        self.crystals -= price.crystals;
    }

    /// One piece of this kind.
    pub fn one(kind: PieceKind) -> Purse {
        match kind {
            PieceKind::Scrap => Purse { scrap: 1, crystals: 0 },
            PieceKind::Crystal => Purse { scrap: 0, crystals: 1 },
        }
    }

    /// "12 scrap, 1 crystal".
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.scrap > 0 || self.crystals == 0 {
            parts.push(format!("{} scrap", self.scrap));
        }
        if self.crystals > 0 {
            parts.push(format!("{} crystal{}", self.crystals, if self.crystals == 1 { "" } else { "s" }));
        }
        parts.join(", ")
    }
}

/// Percent of mass a piece in the hold adds to the ship.
pub const SCRAP_WEIGHT: u32 = 5;
pub const CRYSTAL_WEIGHT: u32 = 15;

/// What can be bought for a ship, in tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Upgrade {
    /// Thrust.
    Engine,
    /// Delta-v.
    Tank,
    /// Shells carried.
    Magazine,
    /// Shells fired and coming back sooner.
    Reload,
    /// Faster shells.
    Guns,
    /// The base takes less damage.
    Armour,
    /// A wider home zone.
    Zone,
    /// New ships sooner.
    Shipyard,
}

/// Tiers above the bare ship.
pub const TIERS: u8 = 3;

impl Upgrade {
    pub const SHIP: [Upgrade; 5] = [Upgrade::Engine, Upgrade::Tank, Upgrade::Magazine, Upgrade::Reload, Upgrade::Guns];
    pub const BASE: [Upgrade; 3] = [Upgrade::Armour, Upgrade::Zone, Upgrade::Shipyard];

    /// For the base, kept for the whole round (not for the ship, and lost a tier with it).
    pub fn for_base(self) -> bool {
        Self::BASE.contains(&self)
    }

    pub fn name(self) -> &'static str {
        match self {
            Upgrade::Engine => "engine",
            Upgrade::Tank => "tank",
            Upgrade::Magazine => "magazine",
            Upgrade::Reload => "reload",
            Upgrade::Guns => "guns",
            Upgrade::Armour => "armour",
            Upgrade::Zone => "zone",
            Upgrade::Shipyard => "shipyard",
        }
    }

    /// What each tier gives, bare ship first.
    pub fn tiers(self) -> [&'static str; TIERS as usize + 1] {
        match self {
            Upgrade::Engine => ["thrust x1", "thrust x1.3", "thrust x1.6", "thrust x2"],
            Upgrade::Tank => ["20 km/s", "26 km/s", "32 km/s", "40 km/s"],
            Upgrade::Magazine => ["10 shells", "15 shells", "20 shells", "30 shells"],
            Upgrade::Reload => ["x1", "x1.3 faster", "x1.7 faster", "x2.2 faster"],
            Upgrade::Guns => ["8 km/s", "10 km/s", "12.5 km/s", "16 km/s"],
            Upgrade::Armour => ["full damage", "75 % damage", "60 % damage", "45 % damage"],
            Upgrade::Zone => ["640 Mm", "800 Mm", "960 Mm", "1120 Mm"],
            Upgrade::Shipyard => ["ship in 5 s", "in 3.5 s", "in 2.5 s", "in 1.5 s"],
        }
    }

    /// What tier `tier` (1 to 3) costs.
    pub fn price(self, tier: u8) -> Purse {
        match (self.for_base(), tier) {
            (true, 1) => Purse { scrap: 15, crystals: 0 },
            (true, 2) => Purse { scrap: 35, crystals: 0 },
            (true, _) => Purse { scrap: 60, crystals: 3 },
            (false, 1) => Purse { scrap: 10, crystals: 0 },
            (false, 2) => Purse { scrap: 25, crystals: 0 },
            (false, _) => Purse { scrap: 50, crystals: 3 },
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Each upgrade's tier, 0 for none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upgrades(pub [u8; 8]);

impl Upgrades {
    pub fn tier(&self, u: Upgrade) -> u8 {
        self.0[u.index()]
    }

    pub fn raise(&mut self, u: Upgrade) {
        self.0[u.index()] = (self.0[u.index()] + 1).min(TIERS);
    }

    /// These tiers for the ship, and those of `base` for the base.
    pub fn with_base(&self, base: &Upgrades) -> Upgrades {
        let mut ups = *self;
        for u in Upgrade::BASE {
            ups.0[u.index()] = base.0[u.index()];
        }
        ups
    }

    /// One tier less of everything for the ship (when it is lost).
    pub fn lower_ship(&mut self) {
        for u in Upgrade::SHIP {
            self.0[u.index()] = self.0[u.index()].saturating_sub(1);
        }
    }

    /// Percent of the bare ship's figure at this upgrade's tier.
    fn percent(&self, u: Upgrade, table: [i64; TIERS as usize + 1]) -> i64 {
        table[self.tier(u) as usize]
    }
}

/// The engine and tank of a ship with these upgrades and this much in its hold.
pub fn kit(rules: &GameRules, ups: &Upgrades, hold: &Purse) -> Kit {
    let bare = Kit::of(rules);
    let burn = bare.burn * ups.percent(Upgrade::Engine, [100, 130, 160, 200]) / 100;
    let tank = bare.tank * ups.percent(Upgrade::Tank, [100, 130, 160, 200]) / 100;
    let weight = 100 + (SCRAP_WEIGHT * hold.scrap + CRYSTAL_WEIGHT * hold.crystals) as i64;
    Kit { burn, push: burn * 100 / weight, tank }
}

/// A full magazine for these upgrades; `quick` (from ammo) fires and reloads twice as fast,
/// with twice the shells.
pub fn magazine(tick: Tick, rules: &GameRules, ups: &Upgrades, quick: bool) -> Magazine {
    let max = rules.shell_max * ups.percent(Upgrade::Magazine, [100, 150, 200, 300]) as u32 / 100;
    let per = ups.percent(Upgrade::Reload, [100, 75, 60, 45]) as u32;
    let (cooldown, regen) = (rules.shell_cooldown_ticks * per / 100, rules.shell_regen_ticks * per / 100);
    if quick {
        Magazine::new(tick, 2 * max, cooldown / 2, regen / 2)
    } else {
        Magazine::new(tick, max, cooldown, regen)
    }
}

/// The fastest a shell leaves the guns (m/s).
pub fn shell_speed_max(rules: &GameRules, ups: &Upgrades) -> f64 {
    rules.shell_speed_max * ups.percent(Upgrade::Guns, [100, 125, 156, 200]) as f64 / 100.0
}

/// How far around the base its home zone reaches, in base radii.
pub fn zone_radii(ups: &Upgrades) -> f64 {
    crate::HOME_RADII * ups.percent(Upgrade::Zone, [100, 125, 150, 175]) as f64 / 100.0
}

/// What is left of `amount` of damage to a base with these upgrades.
pub fn armoured(amount: u32, ups: &Upgrades) -> u32 {
    (amount as i64 * ups.percent(Upgrade::Armour, [100, 75, 60, 45]) / 100) as u32
}

/// Ticks before a lost ship is replaced.
pub fn respawn_ticks(rules: &GameRules, ups: &Upgrades) -> u32 {
    (rules.respawn_ticks as i64 * ups.percent(Upgrade::Shipyard, [100, 70, 50, 30]) / 100) as u32
}

/// What the shop sells once at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Supply {
    /// The ship survives one shell.
    Shield,
    /// Twice the shells, fired and coming back twice as fast, until the ship is lost.
    Ammo,
    /// A fifth of the base's health back.
    Repair,
    /// Carried: a jump back home, leaving the cargo behind.
    Warp,
    /// Carried: dropped where the ship is, it pulls every ship near it in for a while.
    Well,
}

/// The most of each carried supply a player can have.
pub const CARRY: u8 = 2;

impl Supply {
    pub const ALL: [Supply; 5] = [Supply::Shield, Supply::Ammo, Supply::Repair, Supply::Warp, Supply::Well];

    pub fn name(self) -> &'static str {
        match self {
            Supply::Shield => "shield",
            Supply::Ammo => "ammo",
            Supply::Repair => "repair",
            Supply::Warp => "warp home",
            Supply::Well => "gravity well",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Supply::Shield => "survive one shell",
            Supply::Ammo => "2x shells, 2x as fast",
            Supply::Repair => "+20 % base health",
            Supply::Warp => "jump home, cargo stays",
            Supply::Well => "pulls ships in for 6 s",
        }
    }

    pub fn price(self) -> Purse {
        match self {
            Supply::Shield => Purse { scrap: 12, crystals: 0 },
            Supply::Ammo => Purse { scrap: 10, crystals: 0 },
            Supply::Repair => Purse { scrap: 20, crystals: 0 },
            Supply::Warp => Purse { scrap: 15, crystals: 0 },
            Supply::Well => Purse { scrap: 10, crystals: 2 },
        }
    }

    /// Carried, to be used later (the others take effect when bought).
    pub fn carried(self) -> bool {
        matches!(self, Supply::Warp | Supply::Well)
    }
}

/// Supplies carried: warps home and gravity wells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Items {
    pub warps: u8,
    pub wells: u8,
}

impl Items {
    pub fn count(&self, s: Supply) -> u8 {
        match s {
            Supply::Warp => self.warps,
            Supply::Well => self.wells,
            _ => 0,
        }
    }

    pub fn slot(&mut self, s: Supply) -> Option<&mut u8> {
        match s {
            Supply::Warp => Some(&mut self.warps),
            Supply::Well => Some(&mut self.wells),
            _ => None,
        }
    }
}

/// Anything the shop sells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ware {
    Upgrade(Upgrade),
    Supply(Supply),
}
