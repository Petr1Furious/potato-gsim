//! World presets. Generated once on the server; clients receive the result verbatim.

use crate::rng::Rng;
use gsim_core::Body;
use std::f64::consts::PI;

const G: f64 = 6.67430e-11;
/// Earth-like bulk density (kg/m^3) used to size generated bodies.
const DENSITY: f64 = 5514.0;

/// How a parameter is best edited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// A whole number.
    Count,
    Linear,
    /// Spans orders of magnitude: a slider should be logarithmic.
    Log,
}

/// One named number a preset can be tuned with.
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    /// What `--set` and `/preset` call it.
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    /// `"m"`, `"kg"` or empty.
    pub unit: &'static str,
    pub kind: ParamKind,
    pub default: f64,
    pub min: f64,
    pub max: f64,
}

pub struct Preset {
    pub name: &'static str,
    pub about: &'static str,
    pub params: &'static [ParamSpec],
}

/// Settings for a preset by parameter key; keys left out take their defaults.
pub type Params = std::collections::BTreeMap<String, f64>;

const fn count(key: &'static str, label: &'static str, help: &'static str, default: f64, min: f64, max: f64) -> ParamSpec {
    ParamSpec { key, label, help, unit: "", kind: ParamKind::Count, default, min, max }
}

const fn linear(key: &'static str, label: &'static str, help: &'static str, unit: &'static str, default: f64, min: f64, max: f64) -> ParamSpec {
    ParamSpec { key, label, help, unit, kind: ParamKind::Linear, default, min, max }
}

const fn log(key: &'static str, label: &'static str, help: &'static str, unit: &'static str, default: f64, min: f64, max: f64) -> ParamSpec {
    ParamSpec { key, label, help, unit, kind: ParamKind::Log, default, min, max }
}

/// No preset may exceed this many bodies (the ranges below are chosen to guarantee it).
pub const MAX_BODIES: usize = 5000;

/// Parameters of the field presets, which differ only in their defaults.
const fn field(spread: f64, mass_max: f64, rotation: f64, star_mass: f64) -> [ParamSpec; 6] {
    [
        count("count", "Bodies", "Number of bodies, the star included", 1000.0, 1.0, 5000.0),
        log("spread", "Radius", "Radius of the disc the bodies start in", "m", spread, 1.0e9, 1.0e13),
        log("mass_min", "Lightest body", "Masses are drawn log-uniformly between the lightest and the heaviest", "kg", 1.0e21, 1.0e15, 1.0e28),
        log("mass_max", "Heaviest body", "Raised to the lightest mass if it is set below it", "kg", mass_max, 1.0e15, 1.0e30),
        linear("rotation", "Rotation", "Fraction of circular speed given to each body (1 = orbiting, 0 = falls inwards)", "", rotation, 0.0, 1.5),
        linear("star_mass", "Star mass", "Mass of a central star that holds the field together (0 = no star)", "kg", star_mass, 0.0, 5.0e30),
    ]
}

const RANDOM: [ParamSpec; 6] = field(2.0e10, 5.0e24, 0.7, 0.0);
const REPULSOR: [ParamSpec; 7] = [
    RANDOM[0],
    RANDOM[1],
    RANDOM[2],
    RANDOM[3],
    RANDOM[4],
    RANDOM[5],
    linear("negative", "Negative share", "Fraction of the bodies given negative mass", "", 0.25, 0.0, 0.9),
];

pub const PRESETS: &[Preset] = &[
    Preset { name: "random", about: "Rotating field of many bodies that clump and merge (default)", params: &RANDOM },
    Preset { name: "repulsor", about: "Random field where a quarter of the bodies have negative mass", params: &REPULSOR },
];

pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

fn known_preset(name: &str) -> Result<&'static Preset, String> {
    preset(name).ok_or_else(|| {
        let known: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        format!("unknown preset '{name}' (known: {})", known.join(", "))
    })
}

/// A value as short as it can be written and still parse back: `50`, `0.7`, `2e25`.
pub fn format_value(value: f64) -> String {
    if value != 0.0 && (value.abs() >= 1.0e6 || value.abs() < 1.0e-3) {
        format!("{value:e}")
    } else {
        format!("{value}")
    }
}

impl ParamSpec {
    /// `1e9 to 1e13 m`
    pub fn range(&self) -> String {
        format!("{} to {} {}", format_value(self.min), format_value(self.max), self.unit).trim_end().to_string()
    }
}

impl Preset {
    pub fn param(&self, key: &str) -> Option<&'static ParamSpec> {
        self.params.iter().find(|p| p.key == key)
    }

    /// Every parameter at its default.
    pub fn defaults(&self) -> Params {
        self.params.iter().map(|p| (p.key.to_string(), p.default)).collect()
    }
}

/// Parse `key=value`. The value is a plain number (`2e25`) or a length with a unit (`50Gm`).
pub fn parse_setting(text: &str) -> Result<(String, f64), String> {
    let (key, value) = text.split_once('=').filter(|(k, _)| !k.is_empty()).ok_or_else(|| format!("expected key=value, got '{text}'"))?;
    let number = gsim_proto::command::parse_metres(value).ok_or_else(|| format!("{key}='{value}' is not a number"))?;
    Ok((key.to_string(), number))
}

/// Check that every key is a parameter of this preset and every value is allowed.
pub fn validate(preset: &str, params: &Params) -> Result<(), String> {
    let p = known_preset(preset)?;
    for (key, value) in params {
        let Some(spec) = p.param(key) else {
            let known: Vec<&str> = p.params.iter().map(|s| s.key).collect();
            return Err(format!("preset {preset} has no parameter '{key}' (it has: {})", known.join(", ")));
        };
        if !(value.is_finite() && (spec.min..=spec.max).contains(value)) {
            return Err(format!("{key}={} is out of range ({})", format_value(*value), spec.range()));
        }
        if spec.kind == ParamKind::Count && value.fract() != 0.0 {
            return Err(format!("{key}={} must be a whole number", format_value(*value)));
        }
    }
    Ok(())
}

/// The settings that differ from the preset's defaults, as `key=value` words.
pub fn describe(preset: &str, params: &Params) -> String {
    let Some(p) = self::preset(preset) else { return String::new() };
    let changed = p.params.iter().filter_map(|s| params.get(s.key).filter(|v| **v != s.default).map(|v| format!("{}={}", s.key, format_value(*v))));
    changed.collect::<Vec<_>>().join(" ")
}

/// The numbers a preset is built from: what was set, else the defaults.
struct Values<'a> {
    preset: &'static Preset,
    params: &'a Params,
}

impl Values<'_> {
    fn get(&self, key: &str) -> f64 {
        let spec = self.preset.param(key).expect("parameter in the preset's table");
        self.params.get(key).copied().unwrap_or(spec.default)
    }

    fn count(&self, key: &str) -> usize {
        self.get(key) as usize
    }
}

#[derive(Clone)]
pub struct Scenario {
    pub name: String,
    pub bodies: Vec<Body>,
    pub names: Vec<(u32, String)>,
    /// Ships spawn on a circular orbit at a radius in this range around the barycentre.
    pub spawn_r: (f64, f64),
}

pub fn radius_from_mass(mass: f64) -> f64 {
    (3.0 * mass.abs() / (4.0 * PI * DENSITY)).cbrt()
}

fn body(x: f64, y: f64, vx: f64, vy: f64, mass: f64, radius: f64) -> Body {
    Body { x, y, vx, vy, mass, radius }
}

pub fn build(preset: &str, seed: u64, params: &Params) -> Result<Scenario, String> {
    validate(preset, params)?;
    let v = Values { preset: known_preset(preset)?, params };
    let mut rng = Rng::new(seed);
    let mut sc = match preset {
        "random" => random_field(&mut rng, &v, 0.0),
        "repulsor" => random_field(&mut rng, &v, v.get("negative")),
        other => unreachable!("preset {other} is in the table but has no generator"),
    };
    debug_assert!(sc.bodies.len() <= MAX_BODIES);
    sc.name = preset.to_string();
    Ok(sc)
}

fn random_field(rng: &mut Rng, v: &Values, negative_frac: f64) -> Scenario {
    let n = v.count("count");
    let spread = v.get("spread");
    let star = v.get("star_mass");
    let (mass_min, rotation) = (v.get("mass_min"), v.get("rotation"));
    let mass_max = v.get("mass_max").max(mass_min * 1.0001);
    let has_star = star > 0.0 && n > 1;
    // With a star the bodies form a ring around it; without one they fill the whole disc.
    let inner = if has_star { 0.15 } else { 0.0 };
    let mut bodies = Vec::with_capacity(n);
    let mut names = Vec::new();
    if has_star {
        // Sun-like density.
        let radius = (3.0 * star / (4.0 * PI * 1408.0)).cbrt();
        bodies.push(body(0.0, 0.0, 0.0, 0.0, star, radius));
        names.push((0, "Star".to_string()));
    }
    let mut disc = 0.0;
    while bodies.len() < n {
        let a = rng.angle();
        let r = (inner * inner + rng.f64() * (1.0 - inner * inner)).sqrt() * spread;
        let mut mass = rng.log_range(mass_min, mass_max);
        let radius = radius_from_mass(mass);
        if rng.f64() < negative_frac {
            mass = -mass;
        }
        disc += mass;
        bodies.push(body(r * a.cos(), r * a.sin(), 0.0, 0.0, mass, radius));
    }
    // Circular speed from the star plus the share of the (uniform) disc inside each radius,
    // with a little scatter so orbits are not perfectly round.
    for b in bodies.iter_mut().skip(has_star as usize) {
        let r = (b.x * b.x + b.y * b.y).sqrt();
        let u = r / spread;
        let enclosed = star + disc * ((u * u - inner * inner) / (1.0 - inner * inner)).clamp(0.0, 1.0);
        if r > 0.0 && enclosed > 0.0 {
            let scatter = if has_star { rng.range(0.97, 1.03) } else { 1.0 };
            let v = rotation * (G * enclosed / r).sqrt() * scatter;
            b.vx = -v * b.y / r;
            b.vy = v * b.x / r;
        }
    }
    remove_net_momentum(&mut bodies);
    let spawn_r = if has_star { (0.3 * spread, 0.85 * spread) } else { (0.25 * spread, 0.6 * spread) };
    Scenario { name: String::new(), bodies, names, spawn_r }
}

fn remove_net_momentum(bodies: &mut [Body]) {
    let m: f64 = bodies.iter().map(|b| b.mass).sum();
    if m.abs() < 1.0 {
        return;
    }
    let px: f64 = bodies.iter().map(|b| b.mass * b.vx).sum();
    let py: f64 = bodies.iter().map(|b| b.mass * b.vy).sum();
    for b in bodies {
        b.vx -= px / m;
        b.vy -= py / m;
    }
}

impl Scenario {
    /// Largest distance of any body from the origin at the start.
    pub fn extent(&self) -> f64 {
        self.bodies.iter().map(|b| (b.x * b.x + b.y * b.y).sqrt()).fold(0.0, f64::max)
    }

    /// Beyond this, a body that is unbound and heading outwards is gone for good.
    pub fn escape_radius(&self) -> f64 {
        4.0 * self.extent().max(1.0e9)
    }
}
