//! Human-readable quantities.

fn scaled(v: f64, units: &[(f64, &str)]) -> String {
    let a = v.abs();
    let (scale, unit) = units.iter().rev().find(|u| a >= u.0).unwrap_or(&units[0]);
    format!("{:.3} {unit}", v / scale)
}

pub fn distance(m: f64) -> String {
    scaled(m, &[(1.0, "m"), (1e3, "km"), (1e6, "Mm"), (1e9, "Gm"), (1e12, "Tm")])
}

pub fn speed(mps: f64) -> String {
    scaled(mps, &[(1.0, "m/s"), (1e3, "km/s")])
}


pub fn mass(kg: f64) -> String {
    format!("{kg:.3e} kg")
}
