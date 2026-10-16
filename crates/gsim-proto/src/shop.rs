//! What players gather, carry home and spend in the bases mode, and what it buys. Shared, so
//! that both sides work out a ship's engine and guns the same way.

use gsim_core::{GameRules, Kit, Magazine, Tick};
use serde::{Deserialize, Serialize};

/// Something to pick up, floating free: flying close enough takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PieceKind {
    Scrap,
    Crystal,
    /// A supply crate: a random supply for whoever takes it.
    Crate,
}

/// How close a ship has to pass a piece to take it (m).
pub const PICKUP_RADIUS: f64 = 8.0e7;
/// Seconds a piece takes to fade away once it has drifted far out with nobody near.
pub const FADE_SECONDS: f64 = 6.0;

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
            PieceKind::Crate => Purse::default(),
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
    Armor,
    /// A wider home zone.
    HomeZone,
    /// New ships sooner.
    Respawn,
}

/// Tiers above the bare ship.
pub const TIERS: u8 = 3;

impl Upgrade {
    pub const SHIP: [Upgrade; 5] = [Upgrade::Thrust, Upgrade::DeltaV, Upgrade::Shells, Upgrade::FireRate, Upgrade::ShellSpeed];
    pub const BASE: [Upgrade; 3] = [Upgrade::Armor, Upgrade::HomeZone, Upgrade::Respawn];

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
            Upgrade::Armor => "armor",
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
            Upgrade::Armor => times(ups.percent(self, ARMOR)),
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

/// Thrust, fire rate and armor, in percent of the bare figure, by tier.
const THRUST: [i64; 4] = [100, 150, 200, 300];
const FIRE_RATE: [i64; 4] = [100, 150, 200, 300];
const ARMOR: [i64; 4] = [100, 150, 200, 300];

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
pub fn armored(amount: u32, ups: &Upgrades) -> u32 {
    (amount as i64 * 100 / ups.percent(Upgrade::Armor, ARMOR)) as u32
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
    /// Carried: the engines of enemy ships near this one go dead for a moment.
    Emp,
    /// Carried: for a while the ship takes scrap and crystals from much further away.
    Magnet,
    /// Carried: more delta-v, beyond a full tank.
    Afterburner,
    /// Other ships can be selected, to fly to and aim at, until the ship is lost.
    Lock,
}

/// The most of each carried supply a player can have.
pub const CARRY: u8 = 2;

impl Supply {
    pub const ALL: [Supply; 9] = [Supply::Shield, Supply::RapidFire, Supply::Repair, Supply::Lock, Supply::Warp, Supply::Well, Supply::Emp, Supply::Magnet, Supply::Afterburner];
    /// Those carried, in the order of the keys that use them (1 to 5).
    pub const CARRIED: [Supply; 5] = [Supply::Warp, Supply::Well, Supply::Emp, Supply::Magnet, Supply::Afterburner];

    pub fn name(self) -> &'static str {
        match self {
            Supply::Shield => "shield",
            Supply::RapidFire => "rapid fire",
            Supply::Repair => "base repair",
            Supply::Warp => "warp home",
            Supply::Well => "gravity well",
            Supply::Emp => "EMP",
            Supply::Magnet => "scrap magnet",
            Supply::Afterburner => "afterburner",
            Supply::Lock => "target lock",
        }
    }

    /// How commands name it.
    pub fn key(self) -> String {
        self.name().to_lowercase().replace(' ', "-")
    }

    pub fn price(self) -> Purse {
        match self {
            Supply::Shield => Purse { scrap: 5, crystals: 0 },
            Supply::RapidFire => Purse { scrap: 4, crystals: 0 },
            Supply::Repair => Purse { scrap: 8, crystals: 0 },
            Supply::Warp => Purse { scrap: 6, crystals: 0 },
            Supply::Well => Purse { scrap: 4, crystals: 1 },
            Supply::Emp => Purse { scrap: 6, crystals: 1 },
            Supply::Magnet => Purse { scrap: 3, crystals: 0 },
            Supply::Afterburner => Purse { scrap: 5, crystals: 0 },
            Supply::Lock => Purse { scrap: 12, crystals: 3 },
        }
    }

    /// Carried, to be used later (the others take effect when bought).
    pub fn carried(self) -> bool {
        Self::CARRIED.contains(&self)
    }
}

/// How many of each carried supply (see [`Supply::CARRIED`]) a player has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Items(pub [u8; 5]);

impl Items {
    pub fn count(&self, s: Supply) -> u8 {
        Supply::CARRIED.iter().position(|c| *c == s).map_or(0, |k| self.0[k])
    }

    pub fn slot(&mut self, s: Supply) -> Option<&mut u8> {
        Supply::CARRIED.iter().position(|c| *c == s).map(|k| &mut self.0[k])
    }
}

/// What an EMP reaches (m), and for how long the engines it hits stay dead (s).
pub const EMP_RADIUS: f64 = 1.2e9;
pub const EMP_SECONDS: f64 = 3.0;
/// How long a scrap magnet lasts (s), and how many times as far it reaches.
pub const MAGNET_SECONDS: f64 = 30.0;
pub const MAGNET_REACH: f64 = 3.0;
/// What an afterburner adds (mm/s of delta-v).
pub const AFTERBURNER_MMPS: i64 = 10_000_000;

/// Anything the shop sells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ware {
    Upgrade(Upgrade),
    Supply(Supply),
}
