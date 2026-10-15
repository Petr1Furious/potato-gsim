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
    Thrust,
    DeltaV,
    /// Shells carried.
    Shells,
    /// How fast shells are fired and come back.
    FireRate,
    ShellSpeed,
    /// The base takes less damage.
    Armour,
    /// A wider home zone.
    HomeZone,
    /// New ships sooner.
    Respawn,
}

/// Tiers above the bare ship.
pub const TIERS: u8 = 3;

impl Upgrade {
    pub const SHIP: [Upgrade; 5] = [Upgrade::Thrust, Upgrade::DeltaV, Upgrade::Shells, Upgrade::FireRate, Upgrade::ShellSpeed];
    pub const BASE: [Upgrade; 3] = [Upgrade::Armour, Upgrade::HomeZone, Upgrade::Respawn];

    /// For the base (shared by its team), rather than for the ship.
    pub fn for_base(self) -> bool {
        Self::BASE.contains(&self)
    }

    pub fn name(self) -> &'static str {
        match self {
            Upgrade::Thrust => "thrust",
            Upgrade::DeltaV => "delta-v",
            Upgrade::Shells => "shells",
            Upgrade::FireRate => "fire rate",
            Upgrade::ShellSpeed => "shell speed",
            Upgrade::Armour => "armour",
            Upgrade::HomeZone => "home zone",
            Upgrade::Respawn => "respawn",
        }
    }

    /// What tier `tier` (0 for none) amounts to, under these rules: "x1.5", "30 km/s".
    pub fn tier_text(self, rules: &GameRules, tier: u8) -> String {
        let ups = Upgrades::default().at(self, tier);
        let times = |pct: i64| format!("x{}", pct as f64 / 100.0);
        match self {
            Upgrade::Thrust => times(ups.percent(self, THRUST)),
            Upgrade::DeltaV => format!("{} km/s", kit(rules, &ups, &Purse::default()).tank / 1_000_000),
            Upgrade::Shells => magazine(0, rules, &ups, false).max.to_string(),
            Upgrade::FireRate => times(ups.percent(self, FIRE_RATE)),
            Upgrade::ShellSpeed => format!("{} km/s", shell_speed_max(rules, &ups) / 1000.0),
            Upgrade::Armour => times(ups.percent(self, ARMOUR)),
            Upgrade::HomeZone => format!("{:.0} Mm", zone_radii(&ups) * crate::BASE_RADIUS / 1.0e6),
            Upgrade::Respawn => format!("{} s", respawn_ticks(rules, &ups) as f64 / rules.tick_hz as f64),
        }
    }

    /// What tier `tier` (1 to 3) costs.
    pub fn price(self, tier: u8) -> Purse {
        match (self.for_base(), tier) {
            (true, 1) => Purse { scrap: 6, crystals: 0 },
            (true, 2) => Purse { scrap: 12, crystals: 0 },
            (true, _) => Purse { scrap: 24, crystals: 4 },
            (false, 1) => Purse { scrap: 4, crystals: 0 },
            (false, 2) => Purse { scrap: 8, crystals: 0 },
            (false, _) => Purse { scrap: 16, crystals: 4 },
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

    /// The same with this tier of `u`.
    fn at(mut self, u: Upgrade, tier: u8) -> Upgrades {
        self.0[u.index()] = tier.min(TIERS);
        self
    }

    /// Percent of the bare ship's figure at this upgrade's tier.
    fn percent(&self, u: Upgrade, table: [i64; TIERS as usize + 1]) -> i64 {
        table[self.tier(u) as usize]
    }
}

/// Thrust, fire rate and armour, in percent of the bare figure, by tier.
const THRUST: [i64; 4] = [100, 150, 200, 300];
const FIRE_RATE: [i64; 4] = [100, 150, 200, 300];
const ARMOUR: [i64; 4] = [100, 150, 200, 300];

/// The engine and tank of a ship with these upgrades and this much in its hold.
pub fn kit(rules: &GameRules, ups: &Upgrades, hold: &Purse) -> Kit {
    let bare = Kit::of(rules);
    let burn = bare.burn * ups.percent(Upgrade::Thrust, THRUST) / 100;
    let tank = bare.tank * ups.percent(Upgrade::DeltaV, [100, 150, 225, 300]) / 100;
    let weight = 100 + (SCRAP_WEIGHT * hold.scrap + CRYSTAL_WEIGHT * hold.crystals) as i64;
    Kit { burn, push: burn * 100 / weight, tank }
}

/// A full magazine for these upgrades; `quick` (rapid fire) fires and reloads twice as fast,
/// with twice the shells.
pub fn magazine(tick: Tick, rules: &GameRules, ups: &Upgrades, quick: bool) -> Magazine {
    let max = rules.shell_max * ups.percent(Upgrade::Shells, [100, 200, 300, 500]) as u32 / 100;
    let rate = ups.percent(Upgrade::FireRate, FIRE_RATE) as u32;
    let (cooldown, regen) = (rules.shell_cooldown_ticks * 100 / rate, rules.shell_regen_ticks * 100 / rate);
    if quick {
        Magazine::new(tick, 2 * max, cooldown / 2, regen / 2)
    } else {
        Magazine::new(tick, max, cooldown, regen)
    }
}

/// The fastest a shell leaves the guns (m/s).
pub fn shell_speed_max(rules: &GameRules, ups: &Upgrades) -> f64 {
    rules.shell_speed_max * ups.percent(Upgrade::ShellSpeed, [100, 150, 200, 300]) as f64 / 100.0
}

/// How far around the base its home zone reaches, in base radii.
pub fn zone_radii(ups: &Upgrades) -> f64 {
    crate::HOME_RADII * ups.percent(Upgrade::HomeZone, [100, 150, 200, 250]) as f64 / 100.0
}

/// What is left of `amount` of damage to a base with these upgrades.
pub fn armoured(amount: u32, ups: &Upgrades) -> u32 {
    (amount as i64 * 100 / ups.percent(Upgrade::Armour, ARMOUR)) as u32
}

/// Ticks before a lost ship is replaced.
pub fn respawn_ticks(rules: &GameRules, ups: &Upgrades) -> u32 {
    (rules.respawn_ticks as i64 * ups.percent(Upgrade::Respawn, [100, 60, 40, 20]) / 100) as u32
}

/// What the shop sells once at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Supply {
    /// The ship survives one shell.
    Shield,
    /// Twice the shells, fired and coming back twice as fast, until the ship is lost.
    RapidFire,
    /// A fifth of the base's health back.
    Repair,
    /// Carried: a jump back home, leaving the cargo behind.
    Warp,
    /// Carried: fired like a shell, it pulls every ship near it in for a while.
    Well,
}

/// The most of each carried supply a player can have.
pub const CARRY: u8 = 2;

impl Supply {
    pub const ALL: [Supply; 5] = [Supply::Shield, Supply::RapidFire, Supply::Repair, Supply::Warp, Supply::Well];

    pub fn name(self) -> &'static str {
        match self {
            Supply::Shield => "shield",
            Supply::RapidFire => "rapid fire",
            Supply::Repair => "base repair",
            Supply::Warp => "warp home",
            Supply::Well => "gravity well",
        }
    }

    /// How commands name it.
    pub fn key(self) -> String {
        self.name().replace(' ', "-")
    }

    pub fn price(self) -> Purse {
        match self {
            Supply::Shield => Purse { scrap: 5, crystals: 0 },
            Supply::RapidFire => Purse { scrap: 4, crystals: 0 },
            Supply::Repair => Purse { scrap: 8, crystals: 0 },
            Supply::Warp => Purse { scrap: 6, crystals: 0 },
            Supply::Well => Purse { scrap: 4, crystals: 1 },
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
