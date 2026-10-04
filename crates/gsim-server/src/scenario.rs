//! World presets. Generated once on the server; clients receive the result verbatim.

use crate::rng::Rng;
use gsim_core::Body;
use std::f64::consts::PI;

const G: f64 = 6.67430e-11;
/// Earth-like bulk density (kg/m^3) used to size generated bodies.
const DENSITY: f64 = 5514.0;

pub const PRESETS: &[(&str, &str)] = &[
    ("random", "Rotating field of many bodies that clump and merge (default)"),
    ("disc", "A star ringed by a crowded disc of orbiting bodies"),
    ("solar", "Sun, eight planets and the Moon"),
    ("binary", "Two stars with a shared debris ring"),
    ("figure8", "Three equal stars chasing each other along a figure-eight"),
    ("repulsor", "Random field where a quarter of the bodies have negative mass"),
    ("runaway", "Star system crossed by a self-accelerating positive/negative pair"),
];

#[derive(Clone, Debug)]
pub struct RandomOpts {
    pub count: usize,
    pub spread: f64,
    pub mass_min: f64,
    pub mass_max: f64,
    /// Fraction of circular speed given to each body (1 = orbiting, 0 = falls inwards).
    pub rotation: f64,
    /// Mass of the central star that holds the field together (0 = no star).
    pub star_mass: f64,
}

impl Default for RandomOpts {
    fn default() -> Self {
        Self { count: 1000, spread: 5.0e10, mass_min: 1.0e21, mass_max: 5.0e25, rotation: 0.7, star_mass: 0.0 }
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

pub fn build(preset: &str, seed: u64, opts: &RandomOpts) -> Result<Scenario, String> {
    let mut rng = Rng::new(seed);
    let mut sc = match preset {
        "random" => random_field(&mut rng, opts, 0.0),
        "disc" => {
            let disc = RandomOpts { spread: 6.0e10, mass_max: 2.0e26, rotation: 1.0, star_mass: 5.0e29, ..opts.clone() };
            random_field(&mut rng, &disc, 0.0)
        }
        "repulsor" => random_field(&mut rng, opts, 0.25),
        "solar" => solar(&mut rng),
        "binary" => binary(&mut rng),
        "figure8" => figure8(&mut rng),
        "runaway" => runaway(&mut rng),
        other => {
            let known: Vec<&str> = PRESETS.iter().map(|p| p.0).collect();
            return Err(format!("unknown preset '{other}' (known: {})", known.join(", ")));
        }
    };
    sc.name = preset.to_string();
    Ok(sc)
}

fn random_field(rng: &mut Rng, opts: &RandomOpts, negative_frac: f64) -> Scenario {
    let n = opts.count.max(1);
    let spread = opts.spread.max(1.0);
    let star = opts.star_mass.max(0.0);
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
        let mut mass = rng.log_range(opts.mass_min.max(1.0), opts.mass_max.max(opts.mass_min.max(1.0) * 1.0001));
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
            let v = opts.rotation * (G * enclosed / r).sqrt() * scatter;
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

fn solar(rng: &mut Rng) -> Scenario {
    let mut bodies = vec![body(0.0, 0.0, 0.0, 0.0, SUN_MASS, SUN_RADIUS)];
    let mut names = vec![(0, "Sun".to_string())];
    for (name, r, mass, radius) in PLANETS {
        let planet = orbiting(SUN_MASS, *r, rng.angle(), *mass, *radius);
        names.push((bodies.len() as u32, name.to_string()));
        bodies.push(planet);
        if *name == "Earth" {
            let d = 3.844e8;
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
    Scenario { name: String::new(), bodies, names, spawn_r: (1.7e11, 2.1e11) }
}

fn binary(rng: &mut Rng) -> Scenario {
    let m = 1.15e30;
    let half = 1.4e11;
    // Equal masses on a common circular orbit: v^2 / half = G m / (2 half)^2.
    let v = (G * m / (4.0 * half)).sqrt();
    let mut bodies = vec![
        body(-half, 0.0, 0.0, -v, m, 6.5e8),
        body(half, 0.0, 0.0, v, m, 6.5e8),
    ];
    for _ in 0..300 {
        let mass = rng.log_range(2.0e22, 4.0e25);
        let r = rng.range(4.5e11, 2.0e12);
        bodies.push(orbiting(2.0 * m, r, rng.angle(), mass, radius_from_mass(mass)));
    }
    let names = vec![(0, "Castor".to_string()), (1, "Pollux".to_string())];
    Scenario { name: String::new(), bodies, names, spawn_r: (5.0e11, 8.0e11) }
}

fn figure8(rng: &mut Rng) -> Scenario {
    // Chenciner-Montgomery choreography in units G = m = 1, scaled to stellar sizes.
    let (m, l) = (1.0e30, 1.0e11);
    let vu = (G * m / l).sqrt();
    let (x1, y1) = (0.97000436 * l, -0.24308753 * l);
    let (v3x, v3y) = (-0.93240737 * vu, -0.86473146 * vu);
    let mut bodies = vec![
        body(x1, y1, -0.5 * v3x, -0.5 * v3y, m, 7.0e8),
        body(-x1, -y1, -0.5 * v3x, -0.5 * v3y, m, 7.0e8),
        body(0.0, 0.0, v3x, v3y, m, 7.0e8),
    ];
    // Featherweight spectators: scenery that cannot disturb the dance.
    for _ in 0..60 {
        let mass = rng.log_range(1.0e14, 1.0e17);
        let r = rng.range(3.5e11, 9.0e11);
        bodies.push(orbiting(3.0 * m, r, rng.angle(), mass, 2.0e6 + rng.f64() * 6.0e6));
    }
    let names = vec![(0, "Alpha".to_string()), (1, "Beta".to_string()), (2, "Gamma".to_string())];
    Scenario { name: String::new(), bodies, names, spawn_r: (3.0e11, 4.5e11) }
}

fn runaway(rng: &mut Rng) -> Scenario {
    let star = 1.5e30;
    let mut bodies = vec![body(0.0, 0.0, 0.0, 0.0, star, 6.5e8)];
    let mut names = vec![(0, "Anchor".to_string())];
    for i in 0..9 {
        let mass = rng.log_range(5.0e23, 2.0e27);
        let r = 6.0e10 * 1.55f64.powi(i);
        bodies.push(orbiting(star, r, rng.angle(), mass, radius_from_mass(mass)));
    }
    for _ in 0..120 {
        let mass = rng.log_range(1.0e19, 1.0e22);
        let r = rng.range(3.0e11, 4.2e11);
        bodies.push(orbiting(star, r, rng.angle(), mass, radius_from_mass(mass)));
    }
    // A positive body is repelled by its negative twin, which in turn is attracted to it:
    // the pair accelerates forever along the line joining them.
    let m = 2.0e27;
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
