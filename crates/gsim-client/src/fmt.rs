//! Human-readable quantities.

fn scaled(v: f64, units: &[(f64, &str)]) -> String {
    let a = v.abs();
    let (scale, unit) = units.iter().rev().find(|u| a >= u.0).unwrap_or(&units[0]);
    format!("{:.3} {unit}", v / scale)
}

pub fn distance(m: f64) -> String {
    scaled(m, &[(1e-3, "mm"), (1.0, "m"), (1e3, "km"), (1e6, "Mm"), (1e9, "Gm"), (1e12, "Tm")])
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
    let units = [(1.0e-3, "ms", 1), (1.0, "s", 1), (60.0, "min", 1), (3600.0, "h", 1), (86_400.0, "d", 1), (31_557_600.0, "y", 1)];
    let (scale, unit, digits) = units.iter().rev().find(|u| seconds >= u.0).unwrap_or(&units[0]);
    let value = seconds / scale;
    // Whole numbers read better without a trailing ".0".
    if (value - value.round()).abs() < 0.05 { format!("{value:.0} {unit}") } else { format!("{value:.digits$} {unit}", digits = *digits as usize) }
}

/// Frame rate and a per-frame cost, averaged over half-second windows so the numbers can be read.
pub struct Meter {
    since: std::time::Instant,
    frames: u32,
    cost: f32,
    pub fps: f32,
    /// Mean of what was passed to [`Meter::frame`] over the last window.
    pub ms: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Self { since: std::time::Instant::now(), frames: 0, cost: 0.0, fps: 0.0, ms: 0.0 }
    }
}

impl Meter {
    pub fn frame(&mut self, cost_ms: f32) {
        self.frames += 1;
        self.cost += cost_ms;
        let span = self.since.elapsed().as_secs_f32();
        if span >= 0.5 {
            self.fps = self.frames as f32 / span;
            self.ms = self.cost / self.frames as f32;
            *self = Self { fps: self.fps, ms: self.ms, ..Self::default() };
        }
    }
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
        assert_eq!(span(0.02), "20 ms");
        assert_eq!(span(2.5), "2.5 s");
        assert_eq!(span(3600.0), "1 h");
        assert_eq!(span(900.0), "15 min");
    }
}
