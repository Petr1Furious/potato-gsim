//! Authoritative game server. [`Authority`] is transport-agnostic (tests drive it over a
//! simulated link); [`net`] runs it on real UDP, and over WebSockets ([`web`]) for browsers.

pub mod authority;
#[cfg(not(target_arch = "wasm32"))]
pub mod net;
pub mod rng;
pub mod scenario;
pub mod state;
#[cfg(not(target_arch = "wasm32"))]
pub mod web;

pub use authority::{Authority, ConnId, Outgoing, Target};
pub use scenario::{Params, Scenario};

/// How much a line of the server log matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }

    pub fn parse(word: &str) -> Option<Level> {
        [Level::Info, Level::Warn, Level::Error].into_iter().find(|l| l.name().eq_ignore_ascii_case(word))
    }
}

/// The least that is logged (see [`set_log_level`]).
static LOG_LEVEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(Level::Info as u8);

pub fn set_log_level(level: Level) {
    LOG_LEVEL.store(level as u8, std::sync::atomic::Ordering::Relaxed);
}

/// A line of the server log: `2026-10-15 17:03:45 INFO  ann joined the game` (UTC), errors to
/// stderr, anything below the level set left out.
pub fn log(level: Level, text: &str) {
    if (level as u8) < LOG_LEVEL.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, mo, d) = civil_date(secs / 86_400);
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    let line = format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02} {:<5} {text}", level.name());
    if level == Level::Error {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

/// Year, month and day of the `days`-th day since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_date(days: u64) -> (i64, u64, u64) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u64;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u64;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::civil_date(0), (1970, 1, 1));
        assert_eq!(super::civil_date(20_376), (2025, 10, 15));
        assert_eq!(super::civil_date(19_782), (2024, 2, 29));
    }
}
