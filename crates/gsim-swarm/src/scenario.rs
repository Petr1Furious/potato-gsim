//! Worlds for the large-scale engine.

use crate::engine::Bodies;
use std::collections::BTreeMap;
use std::f64::consts::{PI, TAU};

const G: f64 = 6.67430e-11;
/// Bulk density (kg/m^3) used to size bodies, as in the multiplayer presets.
const DENSITY: f64 = 5514.0;
const STAR_DENSITY: f64 = 1408.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A whole number.
    Count,
    Linear,
    /// Spans orders of magnitude: sliders should be logarithmic.
    Log,
}

pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
    pub default: f64,
    pub min: f64,
    pub max: f64,
}

pub struct Scenario {
    pub name: &'static str,
    pub about: &'static str,
    pub params: &'static [Param],
}

pub type Params = BTreeMap<String, f64>;

const fn p(key: &'static str, label: &'static str, help: &'static str, unit: &'static str, kind: Kind, default: f64, min: f64, max: f64) -> Param {
    Param { key, label, help, unit, kind, default, min, max }
}

const PACE: Param = p("time_scale", "Pace", "Simulated seconds per second to start with", "x", Kind::Log, 43_200.0, 60.0, 1.0e9);

const COUNT: Param = p("count", "Bodies", "How many bodies the world starts with", "", Kind::Log, 50_000.0, 10.0, 4_000_000.0);
const SIZE: Param = p("size", "Body size", "Multiplies every body's radius: larger bodies collide and merge more", "x", Kind::Log, 0.1, 0.02, 50.0);
const ACCURACY: Param =
    p("accuracy", "Opening angle", "How readily distant groups are treated as one lump: smaller is more accurate and slower", "", Kind::Linear, 0.7, 0.3, 1.2);

const TIGHT: Param = p(
    "tight",
    "Follow tight orbits",
    "1 lets a body that is held tightly (a moon by its planet, a star close to a galaxy's core) take shorter steps of its own, as many as its orbit needs, while everything else takes the world's step; that costs as much as there are such bodies. 0 gives every body the world's step, and whatever turns faster than that comes out wrong",
    "",
    Kind::Count,
    1.0,
    0.0,
    1.0,
);

pub const SCENARIOS: &[Scenario] = &[
    Scenario { name: "empty", about: "Nothing at all: build a world from scratch with the tools", params: &[ACCURACY, TIGHT, PACE] },
    Scenario {
        name: "galaxy",
        about: "A heavy core inside a rotating disc that grows spiral arms and clumps",
        params: &[
            COUNT,
            p("radius", "Disc radius", "Outer edge of the disc", "m", Kind::Log, 1.2e11, 2.0e10, 1.0e12),
            p("core_mass", "Core mass", "Mass of the central body", "kg", Kind::Log, 2.0e29, 1.0e27, 5.0e30),
            p("disc_mass", "Disc mass", "Total mass of everything else: the heavier, the more the disc shapes itself", "kg", Kind::Log, 8.0e28, 1.0e26, 5.0e30),
            p("heat", "Random motion", "Random speed as a fraction of orbital speed: a cold disc clumps, a warm one stays smooth", "", Kind::Linear, 0.03, 0.0, 0.5),
            SIZE,
            ACCURACY,
            TIGHT,
            PACE,
        ],
    },
    Scenario {
        name: "collision",
        about: "Two disc galaxies that pass through each other, throw out tidal tails, fall back and merge (colour by origin, C, tells them apart)",
        params: &[
            COUNT,
            p("radius", "Disc radius", "Outer edge of the larger galaxy", "m", Kind::Log, 8.0e10, 2.0e10, 6.0e11),
            p("core_mass", "Core mass", "Mass of the larger galaxy's core", "kg", Kind::Log, 2.0e29, 1.0e27, 5.0e30),
            p("disc_mass", "Disc mass", "Mass of the larger galaxy's disc: a light disc keeps its shape until the other galaxy pulls it apart, a heavy one breaks up into clumps by itself", "kg", Kind::Log, 1.0e28, 1.0e26, 5.0e30),
            p("ratio", "Mass ratio", "Mass of the second galaxy relative to the first", "", Kind::Linear, 0.6, 0.1, 1.0),
            p("separation", "Separation", "Starting distance between the cores, in disc radii", "R", Kind::Linear, 3.0, 2.5, 10.0),
            p("pass", "Closest pass", "How near the cores come on the first pass, in disc radii", "R", Kind::Linear, 0.5, 0.05, 3.0),
            p("speed", "Approach speed", "Fraction of escape speed: below 1 the pair is bound and falls back", "", Kind::Linear, 0.5, 0.3, 2.0),
            p("retrograde", "Second spins backwards", "1 makes the second galaxy rotate against the orbit, which damps its tails", "", Kind::Count, 0.0, 0.0, 1.0),
            p("heat", "Random motion", "Random speed as a fraction of orbital speed", "", Kind::Linear, 0.08, 0.0, 0.5),
            p("size", "Body size", "Multiplies every body's radius: larger bodies collide and merge more", "x", Kind::Log, 0.1, 0.02, 50.0),
            ACCURACY,
            TIGHT,
            // The galaxies take most of a year to meet and a year to turn once.
            p("time_scale", "Pace", "Simulated seconds per second to start with", "x", Kind::Log, 864_000.0, 60.0, 1.0e9),
        ],
    },
    Scenario {
        name: "cloud",
        about: "A cold, slowly turning cloud that falls together and builds worlds by merging",
        params: &[
            COUNT,
            p("radius", "Cloud radius", "Size of the cloud at the start", "m", Kind::Log, 5.0e10, 5.0e9, 1.0e12),
            p("mass", "Total mass", "Mass of the whole cloud", "kg", Kind::Log, 1.5e29, 1.0e26, 5.0e30),
            p("rotation", "Rotation", "Spin as a fraction of what would hold the cloud up: 0 collapses to a point", "", Kind::Linear, 0.6, 0.0, 1.2),
            p("heat", "Random motion", "Random speed as a fraction of orbital speed at the edge", "", Kind::Linear, 0.04, 0.0, 0.5),
            p("size", "Body size", "Multiplies every body's radius: larger bodies collide and merge more", "x", Kind::Log, 0.6, 0.02, 50.0),
            ACCURACY,
            TIGHT,
            PACE,
        ],
    },
];

pub fn scenario(name: &str) -> Option<&'static Scenario> {
    SCENARIOS.iter().find(|s| s.name == name)
}

/// A world ready to run.
pub struct Setup {
    pub name: String,
    pub bodies: Bodies,
    pub theta: f32,
    /// Whether tight orbits are followed.
    pub tight: bool,
    pub time_scale: f64,
    pub group_names: Vec<&'static str>,
}

/// splitmix64
struct Rng(u64);

impl Rng {
    fn u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn f64(&mut self) -> f64 {
        (self.u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal pair.
    fn normal2(&mut self) -> (f64, f64) {
        let r = (-2.0 * (1.0 - self.f64()).ln()).sqrt();
        let a = self.f64() * TAU;
        (r * a.cos(), r * a.sin())
    }

    /// Log-uniform over two decades around 1.
    fn weight(&mut self) -> f64 {
        (self.f64() * 100f64.ln()).exp() * 0.1
    }
}

pub fn radius_from_mass(mass: f64, density: f64) -> f64 {
    (3.0 * mass.abs() / (4.0 * PI * density)).cbrt()
}

/// Checks keys and ranges; the error names the offending parameter.
pub fn validate(name: &str, params: &Params) -> Result<(), String> {
    let sc = scenario(name).ok_or_else(|| {
        let known: Vec<&str> = SCENARIOS.iter().map(|s| s.name).collect();
        format!("unknown scenario '{name}' (known: {})", known.join(", "))
    })?;
    for (key, value) in params {
        let spec = sc.params.iter().find(|p| p.key == key).ok_or_else(|| {
            let known: Vec<&str> = sc.params.iter().map(|p| p.key).collect();
            format!("'{name}' has no parameter '{key}' (it has: {})", known.join(", "))
        })?;
        if !value.is_finite() || *value < spec.min || *value > spec.max {
            return Err(format!("{key} must be between {} and {}", spec.min, spec.max));
        }
    }
    Ok(())
}

/// Fraction of an exponential disc's mass inside `x` scale lengths.
fn enclosed(x: f64) -> f64 {
    1.0 - (1.0 + x) * (-x).exp()
}

struct Disc {
    centre: (f64, f64),
    velocity: (f64, f64),
    count: usize,
    radius: f64,
    core_mass: f64,
    disc_mass: f64,
    heat: f64,
    /// +1 counter-clockwise, -1 clockwise.
    spin: f64,
    size: f64,
    group: u8,
}

/// A core with an exponential disc on near-circular orbits around it.
fn disc(rng: &mut Rng, d: &Disc, out: &mut Bodies) {
    let first = out.len();
    out.push(d.centre.0, d.centre.1, d.velocity.0, d.velocity.1, d.core_mass, radius_from_mass(d.core_mass, STAR_DENSITY), d.group);
    let n = d.count.saturating_sub(1);
    // Scale length a quarter of the radius; nothing closer than 4 % (orbits there would be
    // too fast for the time step).
    let (scale, edge, inner) = (d.radius / 4.0, 4.0, 0.16);
    let total = enclosed(edge) - enclosed(inner);
    let weights: Vec<f64> = (0..n).map(|_| rng.weight()).collect();
    let unit = d.disc_mass / weights.iter().sum::<f64>().max(1e-300);
    for w in weights {
        // Invert the enclosed-mass curve by bisection.
        let want = enclosed(inner) + rng.f64() * total;
        let (mut lo, mut hi) = (inner, edge);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if enclosed(mid) < want { lo = mid } else { hi = mid }
        }
        let x = 0.5 * (lo + hi);
        let r = x * scale;
        let a = rng.f64() * TAU;
        let inside = d.core_mass + d.disc_mass * (enclosed(x) - enclosed(inner)) / total;
        let v = (G * inside / r).sqrt();
        let (nx, ny) = rng.normal2();
        let (c, s) = (a.cos(), a.sin());
        let vx = -s * v * d.spin + nx * v * d.heat;
        let vy = c * v * d.spin + ny * v * d.heat;
        let mass = w * unit;
        out.push(d.centre.0 + r * c, d.centre.1 + r * s, d.velocity.0 + vx, d.velocity.1 + vy, mass, radius_from_mass(mass, DENSITY) * d.size, d.group);
    }
    settle(out, first, d.velocity);
}

/// Shift the bodies from `first` on so that together they move with `velocity`.
fn settle(out: &mut Bodies, first: usize, velocity: (f64, f64)) {
    let (mut m, mut px, mut py) = (0.0, 0.0, 0.0);
    for i in first..out.len() {
        let w = out.m[i] as f64;
        m += w;
        px += w * out.vx[i];
        py += w * out.vy[i];
    }
    if m <= 0.0 {
        return;
    }
    for i in first..out.len() {
        out.vx[i] += velocity.0 - px / m;
        out.vy[i] += velocity.1 - py / m;
    }
}

/// A cloud in solid-body rotation: `rotation` = 1 would balance gravity at the edge.
#[allow(clippy::too_many_arguments)]
fn cloud(rng: &mut Rng, count: usize, radius: f64, mass: f64, rotation: f64, heat: f64, size: f64, group: u8, out: &mut Bodies) {
    let first = out.len();
    let weights: Vec<f64> = (0..count).map(|_| rng.weight()).collect();
    let unit = mass / weights.iter().sum::<f64>().max(1e-300);
    let omega = rotation * (G * mass / radius.powi(3)).sqrt();
    let jitter = heat * (G * mass / radius).sqrt();
    for w in weights {
        let (r, a) = (radius * rng.f64().sqrt(), rng.f64() * TAU);
        let (x, y) = (r * a.cos(), r * a.sin());
        let (nx, ny) = rng.normal2();
        let m = w * unit;
        out.push(x, y, -omega * y + nx * jitter, omega * x + ny * jitter, m, radius_from_mass(m, DENSITY) * size, group);
    }
    settle(out, first, (0.0, 0.0));
}

/// Something to drop into a running world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Structure {
    /// A core with a rotating disc.
    Galaxy,
    /// A slowly turning cloud with no centre.
    Cloud,
    /// Small bodies circling a body that is already there.
    Ring,
}

impl Structure {
    pub const ALL: [Structure; 3] = [Structure::Galaxy, Structure::Cloud, Structure::Ring];

    pub fn name(self) -> &'static str {
        match self {
            Structure::Galaxy => "galaxy",
            Structure::Cloud => "cloud",
            Structure::Ring => "ring",
        }
    }
}

/// `count` bodies of `mass` in all, within `radius` of the origin and at rest as a whole.
/// A ring is given the mass of the body it circles as `centre`, and does not include it.
pub fn structure(kind: Structure, seed: u64, count: usize, radius: f64, mass: f64, centre: f64, group: u8) -> Bodies {
    let mut rng = Rng(seed ^ 0x5EED_0F5A);
    let mut out = Bodies::default();
    let count = count.max(1);
    match kind {
        Structure::Galaxy => {
            let d = Disc { centre: (0.0, 0.0), velocity: (0.0, 0.0), count, radius, core_mass: 0.7 * mass, disc_mass: 0.3 * mass, heat: 0.03, spin: 1.0, size: 0.1, group };
            disc(&mut rng, &d, &mut out);
        }
        Structure::Cloud => cloud(&mut rng, count, radius, mass, 0.6, 0.04, 0.3, group, &mut out),
        Structure::Ring => {
            let each = mass / count as f64;
            for _ in 0..count {
                // A band a fifth of the radius wide, each body on its own circle.
                let (r, a) = (radius * (0.9 + 0.2 * rng.f64()), rng.f64() * TAU);
                let v = (G * centre.max(1.0) / r).sqrt();
                out.push(r * a.cos(), r * a.sin(), -v * a.sin(), v * a.cos(), each, radius_from_mass(each, DENSITY) * 0.3, group);
            }
        }
    }
    out
}

pub fn build(name: &str, seed: u64, params: &Params) -> Result<Setup, String> {
    validate(name, params)?;
    let sc = scenario(name).unwrap();
    let get = |key: &str| params.get(key).copied().unwrap_or_else(|| sc.params.iter().find(|p| p.key == key).map_or(0.0, |p| p.default));
    let mut rng = Rng(seed ^ 0x5EED_0F5A);
    let count = get("count") as usize;
    let mut bodies = Bodies::default();
    let group_names = match name {
        "galaxy" => {
            let radius = get("radius");
            let d = Disc {
                centre: (0.0, 0.0),
                velocity: (0.0, 0.0),
                count,
                radius,
                core_mass: get("core_mass"),
                disc_mass: get("disc_mass"),
                heat: get("heat"),
                spin: 1.0,
                size: get("size"),
                group: 0,
            };
            disc(&mut rng, &d, &mut bodies);
            vec!["disc"]
        }
        "collision" => {
            let (radius, ratio) = (get("radius"), get("ratio"));
            let (m1, m2) = (get("core_mass") + get("disc_mass"), (get("core_mass") + get("disc_mass")) * ratio);
            let total = m1 + m2;
            let apart = get("separation") * radius;
            // A two-body orbit with the chosen closest pass, entered at the chosen speed.
            let pass = (get("pass") * radius).min(0.9 * apart);
            let speed = get("speed") * (2.0 * G * total / apart).sqrt();
            let sideways = ((2.0 * G * total * pass).sqrt() / apart).min(speed);
            let inwards = (speed * speed - sideways * sideways).max(0.0).sqrt();
            let (f1, f2) = (m2 / total, m1 / total);
            let second = ((count as f64 * ratio / (1.0 + ratio)) as usize).max(2);
            let a = Disc {
                centre: (-apart * f1, 0.0),
                velocity: (inwards * f1, -sideways * f1),
                count: count.saturating_sub(second).max(2),
                radius,
                core_mass: get("core_mass"),
                disc_mass: get("disc_mass"),
                heat: get("heat"),
                spin: 1.0,
                size: get("size"),
                group: 0,
            };
            let b = Disc {
                centre: (apart * f2, 0.0),
                velocity: (-inwards * f2, sideways * f2),
                count: second,
                radius: radius * ratio.sqrt(),
                core_mass: a.core_mass * ratio,
                disc_mass: a.disc_mass * ratio,
                spin: if get("retrograde") >= 0.5 { -1.0 } else { 1.0 },
                group: 1,
                ..a
            };
            disc(&mut rng, &a, &mut bodies);
            disc(&mut rng, &b, &mut bodies);
            vec!["first galaxy", "second galaxy"]
        }
        "empty" => {
            // Nothing: a world to build with the tools. Nothing is ever dropped from it.
            vec![]
        }
        _ => {
            let radius = get("radius");
            cloud(&mut rng, count, radius, get("mass"), get("rotation"), get("heat"), get("size"), 0, &mut bodies);
            vec!["cloud"]
        }
    };
    Ok(Setup {
        name: name.to_string(),
        bodies,
        theta: get("accuracy") as f32,
        tight: get("tight") >= 0.5,
        time_scale: get("time_scale"),
        group_names,
    })
}
