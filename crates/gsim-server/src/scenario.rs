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

const RANDOM: [ParamSpec; 6] = field(5.0e10, 5.0e25, 0.7, 0.0);
const DISC: [ParamSpec; 6] = field(6.0e10, 2.0e26, 1.0, 5.0e29);
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
    Preset { name: "disc", about: "A star ringed by a crowded disc of orbiting bodies", params: &DISC },
    Preset {
        name: "solar",
        about: "Sun, eight planets and the Moon",
        params: &[
            log("star_mass", "Sun mass", "Mass of the Sun; the planets are put on circular orbits around it", "kg", SUN_MASS, 2.0e29, 5.0e30),
            log("scale", "Orbit scale", "Multiplies every orbit radius, the Moon's included", "", 1.0, 0.2, 5.0),
            count("moons", "Moon", "1 gives Earth its Moon, 0 leaves it out", 1.0, 0.0, 1.0),
        ],
    },
    Preset {
        name: "binary",
        about: "Two stars with a shared debris ring",
        params: &[
            log("star_mass", "Star mass", "Mass of each of the two stars", "kg", 1.15e30, 1.0e29, 1.0e31),
            log("separation", "Separation", "Distance between the stars; the debris ring scales with it", "m", 2.8e11, 4.0e10, 2.0e12),
            count("debris", "Debris", "Bodies in the ring around both stars", 300.0, 0.0, 4000.0),
        ],
    },
    Preset {
        name: "figure8",
        about: "Three equal stars chasing each other along a figure-eight",
        params: &[
            log("star_mass", "Star mass", "Mass of each of the three stars", "kg", 1.0e30, 1.0e28, 1.0e31),
            log("size", "Size", "Half-width of the figure-eight; the spectators' orbits scale with it", "m", 1.0e11, 2.0e10, 1.0e12),
            count("spectators", "Spectators", "Featherweight bodies orbiting outside the dance", 60.0, 0.0, 4000.0),
        ],
    },
    Preset { name: "repulsor", about: "Random field where a quarter of the bodies have negative mass", params: &REPULSOR },
    Preset {
        name: "runaway",
        about: "Star system crossed by a self-accelerating positive/negative pair",
        params: &[
            log("star_mass", "Star mass", "Mass of the central star", "kg", 1.5e30, 1.0e29, 1.0e31),
            count("planets", "Planets", "Planets on widening orbits around the star", 9.0, 0.0, 12.0),
            count("debris", "Debris", "Small bodies in the belt between the planets", 120.0, 0.0, 4000.0),
            log("pair_mass", "Pair mass", "Mass of each half of the runaway pair (one positive, one negative)", "kg", 2.0e27, 1.0e24, 1.0e29),
        ],
    },
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

/// Body on a counter-clockwise circular orbit of radius `r` around a central mass at rest.
fn orbiting(central_mass: f64, r: f64, phase: f64, mass: f64, radius: f64) -> Body {
    let v = (G * central_mass / r).sqrt();
    let (s, c) = phase.sin_cos();
    body(r * c, r * s, -v * s, v * c, mass, radius)
}

pub fn build(preset: &str, seed: u64, params: &Params) -> Result<Scenario, String> {
    validate(preset, params)?;
    let v = Values { preset: known_preset(preset)?, params };
    let mut rng = Rng::new(seed);
    let mut sc = match preset {
        "random" | "disc" => random_field(&mut rng, &v, 0.0),
        "repulsor" => random_field(&mut rng, &v, v.get("negative")),
        "solar" => solar(&mut rng, &v),
        "binary" => binary(&mut rng, &v),
        "figure8" => figure8(&mut rng, &v),
        "runaway" => runaway(&mut rng, &v),
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

const SUN_MASS: f64 = 1.98847e30;
const SUN_RADIUS: f64 = 6.9634e8;
/// name, orbit radius (m), mass (kg), radius (m)
const PLANETS: &[(&str, f64, f64, f64)] = &[
    ("Mercury", 5.790905e10, 3.3011e23, 2.4397e6),
    ("Venus", 1.08208e11, 4.8675e24, 6.0518e6),
    ("Earth", 1.49598e11, 5.97237e24, 6.371e6),
    ("Mars", 2.27939e11, 6.4171e23, 3.3895e6),
    ("Jupiter", 7.7857e11, 1.8982e27, 6.9911e7),
    ("Saturn", 1.43353e12, 5.6834e26, 5.8232e7),
    ("Uranus", 2.87246e12, 8.6810e25, 2.5362e7),
    ("Neptune", 4.49506e12, 1.02413e26, 2.4622e7),
];

fn solar(rng: &mut Rng, v: &Values) -> Scenario {
    let (sun, scale, moons) = (v.get("star_mass"), v.get("scale"), v.count("moons"));
    let mut bodies = vec![body(0.0, 0.0, 0.0, 0.0, sun, SUN_RADIUS)];
    let mut names = vec![(0, "Sun".to_string())];
    for (name, r, mass, radius) in PLANETS {
        let planet = orbiting(sun, *r * scale, rng.angle(), *mass, *radius);
        names.push((bodies.len() as u32, name.to_string()));
        bodies.push(planet);
        if *name == "Earth" && moons > 0 {
            // Scaled with the orbits, the Moon stays as deep inside Earth's Hill sphere.
            let d = 3.844e8 * scale;
            let mut moon = orbiting(*mass, d, rng.angle(), 7.342e22, 1.7371e6);
            moon.x += planet.x;
            moon.y += planet.y;
            moon.vx += planet.vx;
            moon.vy += planet.vy;
            names.push((bodies.len() as u32, "Moon".to_string()));
            bodies.push(moon);
        }
    }
    remove_net_momentum(&mut bodies);
    Scenario { name: String::new(), bodies, names, spawn_r: (1.7e11 * scale, 2.1e11 * scale) }
}

fn binary(rng: &mut Rng, v: &Values) -> Scenario {
    let m = v.get("star_mass");
    let half = v.get("separation") / 2.0;
    // The ring was laid out for stars 1.4e11 m from the centre.
    let k = half / 1.4e11;
    // Equal masses on a common circular orbit: v^2 / half = G m / (2 half)^2.
    let speed = (G * m / (4.0 * half)).sqrt();
    let mut bodies = vec![
        body(-half, 0.0, 0.0, -speed, m, 6.5e8),
        body(half, 0.0, 0.0, speed, m, 6.5e8),
    ];
    for _ in 0..v.count("debris") {
        let mass = rng.log_range(2.0e22, 4.0e25);
        let r = rng.range(4.5e11 * k, 2.0e12 * k);
        bodies.push(orbiting(2.0 * m, r, rng.angle(), mass, radius_from_mass(mass)));
    }
    let names = vec![(0, "Castor".to_string()), (1, "Pollux".to_string())];
    Scenario { name: String::new(), bodies, names, spawn_r: (5.0e11 * k, 8.0e11 * k) }
}

fn figure8(rng: &mut Rng, v: &Values) -> Scenario {
    // Chenciner-Montgomery choreography in units G = m = 1, scaled to stellar sizes.
    let (m, l) = (v.get("star_mass"), v.get("size"));
    // The spectators' orbits were laid out for a size of 1e11 m.
    let k = l / 1.0e11;
    let vu = (G * m / l).sqrt();
    let (x1, y1) = (0.97000436 * l, -0.24308753 * l);
    let (v3x, v3y) = (-0.93240737 * vu, -0.86473146 * vu);
    let mut bodies = vec![
        body(x1, y1, -0.5 * v3x, -0.5 * v3y, m, 7.0e8),
        body(-x1, -y1, -0.5 * v3x, -0.5 * v3y, m, 7.0e8),
        body(0.0, 0.0, v3x, v3y, m, 7.0e8),
    ];
    // Featherweight spectators: scenery that cannot disturb the dance.
    for _ in 0..v.count("spectators") {
        let mass = rng.log_range(1.0e14, 1.0e17);
        let r = rng.range(3.5e11 * k, 9.0e11 * k);
        bodies.push(orbiting(3.0 * m, r, rng.angle(), mass, 2.0e6 + rng.f64() * 6.0e6));
    }
    let names = vec![(0, "Alpha".to_string()), (1, "Beta".to_string()), (2, "Gamma".to_string())];
    Scenario { name: String::new(), bodies, names, spawn_r: (3.0e11 * k, 4.5e11 * k) }
}

fn runaway(rng: &mut Rng, v: &Values) -> Scenario {
    let star = v.get("star_mass");
    let mut bodies = vec![body(0.0, 0.0, 0.0, 0.0, star, 6.5e8)];
    let mut names = vec![(0, "Anchor".to_string())];
    for i in 0..v.count("planets") as i32 {
        let mass = rng.log_range(5.0e23, 2.0e27);
        let r = 6.0e10 * 1.55f64.powi(i);
        bodies.push(orbiting(star, r, rng.angle(), mass, radius_from_mass(mass)));
    }
    for _ in 0..v.count("debris") {
        let mass = rng.log_range(1.0e19, 1.0e22);
        let r = rng.range(3.0e11, 4.2e11);
        bodies.push(orbiting(star, r, rng.angle(), mass, radius_from_mass(mass)));
    }
    // A positive body is repelled by its negative twin, which in turn is attracted to it:
    // the pair accelerates forever along the line joining them.
    let m = v.get("pair_mass");
    let (sep, y) = (2.5e9, 2.6e11);
    names.push((bodies.len() as u32, "Hare".to_string()));
    bodies.push(body(-9.0e11, y, 0.0, 0.0, m, radius_from_mass(m)));
    names.push((bodies.len() as u32, "Hound".to_string()));
    bodies.push(body(-9.0e11 - sep, y, 0.0, 0.0, -m, radius_from_mass(m)));
    Scenario { name: String::new(), bodies, names, spawn_r: (1.2e11, 2.2e11) }
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
