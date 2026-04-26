#pragma once

#include <cstdint>

namespace net {

/// Multiplayer driver: **wall-clock** cadence is fixed at 240 Hz (one physics tick per 1/240 s of
/// real time). Each tick integrates **`simulationDtFromTimeScale(timeScale)`** simulated seconds.
/// So total simulated time per real second is always `240 * (timeScale/240) = timeScale`, and
/// step count does **not** grow with `timeScale` (high time speed is not CPU-bound by extra
/// substeps).

/// Wall seconds between physics steps (server + MP client); keep server and client identical.
inline constexpr double kRealSecondsPerPhysicsStep = 1.0 / 240.0;

/// Simulated seconds advanced in one physics step, given `SimulationConfig::timeScale` =
/// simulated seconds per real second.
inline double simulationDtFromTimeScale(const double timeScale) {
	return timeScale / 240.0;
}

/// Must match server and client integration (m/s²).
inline constexpr double kShipThrustAccel = 0.02;

/// Legacy name: same as `kRealSecondsPerPhysicsStep` (fixed wall cadence).
inline constexpr double kPhysicsDt = kRealSecondsPerPhysicsStep;

/// Max physics steps per **render frame** after a hitch (real-step backlog).
inline constexpr int kMaxCatchUpPhysicsStepsPerFrame = 96;
/// Max physics steps per **sim-thread wakeup** (MP client); caps burst catch-up CPU.
inline constexpr int kMaxCatchUpPhysicsStepsPerSimThreadWake = 96;
/// Max client `globalPhysicsStep` lead over last confirmed authority (physics steps).
inline constexpr std::uint64_t kMpMaxClientLeadPhysicsSteps = 480;
/// Max physics steps per **server network tick** (~30 Hz wall).
inline constexpr int kMaxCatchUpPhysicsStepsPerServerTick = 192;
/// Drop excess **wall-clock** backlog so a long stall does not freeze the process (seconds).
inline constexpr double kMaxWallPhysicsDebtSeconds = 0.5;

/// Max client `globalPhysicsStep` lead over last confirmed authority before wall-clock prediction
/// stalls (physics steps; at scale 1 this is ~2 s).
inline constexpr std::uint64_t kMaxPredictionLeadPhysicsSteps = 480;

/// Multiplayer shell weapon tuning.
inline constexpr double kShellRadius = 4.0;
/// Gap between ship hull and shell hull at spawn (world units); shell center is
/// `shipRadius + kShellRadius + kShellMuzzleSurfaceGap` along aim from ship center.
inline constexpr double kShellMuzzleSurfaceGapWorld = kShellRadius;
inline constexpr double kShellMass = 8.0;
inline constexpr double kShellSpeedMin = 1000.0;
inline constexpr double kShellSpeedMax = 8000.0;
inline constexpr double kShellExplosionRadius = 20000000.0;
/// Wall time before a shell may damage **only the ship that fired it** (other ships: no delay).
inline constexpr double kShellArmDelayRealSeconds = 0.50;
inline constexpr double kShellLifetimeRealSeconds = 10.0;
inline constexpr double kShellCooldownRealSeconds = 1.00;

}  // namespace net
