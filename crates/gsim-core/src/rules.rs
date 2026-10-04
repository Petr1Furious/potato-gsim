use serde::{Deserialize, Serialize};

/// Every tunable that influences the simulation. Generated once by the server and sent
/// verbatim to clients at join, so both sides use identical bits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameRules {
    pub tick_hz: u32,
    /// Simulated seconds advanced per tick (`time_scale / tick_hz`).
    pub dt: f64,
    pub g: f64,
    /// Plummer softening between massive bodies (m). Must be > 0.
    pub softening: f64,
    /// Overlapping bodies whose summed mass is below this fraction of the summed |mass|
    /// annihilate instead of merging (only possible with negative masses).
    pub annihilate_frac: f64,
    /// Bodies further than this from the barycentre that are unbound and moving outwards
    /// are removed (0 = never). Set per world by the server.
    pub escape_radius: f64,
    pub escape_check_ticks: u32,

    /// Softening for ship/shell vs body (m); the swept collision test fires long before it matters.
    pub particle_softening: f64,
    /// Sub-step so that `omega * h <= eta` for the strongest attractor.
    pub substep_eta: f64,
    /// Sub-step so that relative travel per sub-step is at most `kappa * distance`.
    pub substep_kappa: f64,
    pub max_substeps: u32,

    pub ship_radius: f64,
    /// Delta-v gained (and fuel spent) per tick at 100 % thrust, in mm/s.
    pub burn_per_tick_mmps: i64,
    pub fuel_max_mmps: i64,
    pub regen_delay_ticks: u32,
    pub regen_per_tick_mmps: i64,

    pub shell_radius: f64,
    pub shell_speed_min: f64,
    pub shell_speed_max: f64,
    pub shell_cooldown_ticks: u32,
    pub shell_lifetime_ticks: u32,
    /// A shell cannot explode before this many ticks after launch (wind-up, applies to everyone).
    pub shell_arm_ticks: u32,
    pub shell_blast_radius: f64,
    pub respawn_ticks: u32,

    /// Inputs are scheduled at least this many ticks ahead of the sender's present.
    pub input_delay_ticks: u32,
    pub max_input_lead_ticks: u32,
    pub hash_interval_ticks: u32,
    pub ship_check_interval_ticks: u32,

    /// Objective: stay on a qualifying orbit around the target for this long to capture it.
    pub hold_ticks: u32,
    pub orbit_max_ecc: f64,
    /// Periapsis must clear the surface by this many body radii, apoapsis must stay within.
    pub orbit_min_peri_radii: f64,
    pub orbit_max_apo_radii: f64,
    pub capture_points: u32,
}

impl GameRules {
    /// `time_scale` is simulated seconds per real second.
    pub fn new(time_scale: f64, tick_hz: u32) -> Self {
        let hz = tick_hz.max(1);
        let dt = time_scale / hz as f64;
        let secs = |s: f64| ((s * hz as f64) + 0.5) as u32;
        let thrust_accel = 0.02; // m/s^2 at 100 %
        Self {
            tick_hz: hz,
            dt,
            g: 6.67430e-11,
            softening: 1.0e6,
            annihilate_frac: 0.05,
            escape_radius: 0.0,
            escape_check_ticks: hz,
            particle_softening: 1.0e3,
            substep_eta: 0.1,
            substep_kappa: 0.5,
            max_substeps: 256,
            ship_radius: 15.0,
            burn_per_tick_mmps: ((thrust_accel * dt * 1000.0) + 0.5) as i64,
            fuel_max_mmps: 10_000_000,
            regen_delay_ticks: secs(5.0),
            regen_per_tick_mmps: 500_000 / hz as i64,
            shell_radius: 4.0,
            shell_speed_min: 1000.0,
            shell_speed_max: 8000.0,
            shell_cooldown_ticks: secs(1.0),
            shell_lifetime_ticks: secs(10.0),
            shell_arm_ticks: secs(0.5),
            shell_blast_radius: 2.0e7,
            respawn_ticks: secs(3.0),
            input_delay_ticks: secs(0.1),
            max_input_lead_ticks: secs(2.0),
            hash_interval_ticks: hz,
            ship_check_interval_ticks: hz / 2,
            hold_ticks: secs(10.0),
            orbit_max_ecc: 0.5,
            orbit_min_peri_radii: 1.5,
            orbit_max_apo_radii: 60.0,
            capture_points: 3,
        }
    }

    pub fn time_scale(&self) -> f64 {
        self.dt * self.tick_hz as f64
    }
}
