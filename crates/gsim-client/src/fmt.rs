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

/// Like [`distance`] but without trailing zeros, for round lengths ("10 Gm", "2.5 Mm").
pub fn distance_round(m: f64) -> String {
    let s = distance(m);
    match s.split_once(' ') {
        Some((n, unit)) if n.contains('.') => format!("{} {unit}", n.trim_end_matches('0').trim_end_matches('.')),
        _ => s,
    }
}

/// A length of time, in the largest unit that keeps the number readable.
pub fn span(seconds: f64) -> String {
    let units = [(1.0, "s", 0), (60.0, "min", 1), (3600.0, "h", 1), (86_400.0, "d", 1), (31_557_600.0, "y", 1)];
    let (scale, unit, digits) = units.iter().rev().find(|u| seconds >= u.0).unwrap_or(&units[0]);
    let value = seconds / scale;
    // Whole numbers read better without a trailing ".0".
    if (value - value.round()).abs() < 0.05 { format!("{value:.0} {unit}") } else { format!("{value:.digits$} {unit}", digits = *digits as usize) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans() {
        assert_eq!(span(86_400.0), "1 d");
        assert_eq!(span(172_800.0), "2 d");
        assert_eq!(span(5400.0), "1.5 h");
        assert_eq!(span(45.0), "45 s");
        assert_eq!(span(3600.0), "1 h");
        assert_eq!(span(900.0), "15 min");
    }
}
