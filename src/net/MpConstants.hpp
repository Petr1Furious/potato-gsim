#pragma once

#include <cstddef>
#include <cstdint>

namespace net {

/// Multiplayer driver: fixed **wall-clock** cadence `1 / realSecondsPerPhysicsStep` Hz. Each step
/// integrates **`simulationDtFromTimeScale(timeScale, realSecondsPerPhysicsStep)`** simulated
/// seconds. Simulated time per real second stays `timeScale` regardless of step rate.

/// Default wall seconds between physics steps (solo client / until JoinAccept in MP).
inline constexpr double kDefaultRealSecondsPerPhysicsStep = 1.0 / 240.0;
/// Default ship forward thrust scale (m/s² per unit thrust); server may override via JoinAccept.
inline constexpr double kDefaultShipThrustAccel = 0.02;

/// Simulated seconds advanced in one physics step, given `SimulationConfig::timeScale` =
/// simulated seconds per real second, and wall step duration `realSecondsPerPhysicsStep`.
inline double simulationDtFromTimeScale(
    const double timeScale,
    const double realSecondsPerPhysicsStep = kDefaultRealSecondsPerPhysicsStep) {
	return timeScale * realSecondsPerPhysicsStep;
}

/// Max physics steps per **render frame** after a hitch (real-step backlog).
inline constexpr int kMaxCatchUpPhysicsStepsPerFrame = 96;
/// Max physics steps per **sim-thread wakeup** (MP client); caps burst catch-up CPU.
inline constexpr int kMaxCatchUpPhysicsStepsPerSimThreadWake = 96;
/// Max client `globalPhysicsStep` lead over `serverPhysicsHeadTarget` before integration stalls
/// (physics steps). Also scales the quadratic MP pace curve vs `(target - head)`.
inline constexpr std::uint64_t kMpLeadCapSteps = 480;
/// Max physics steps per **server network tick** (~30 Hz wall).
inline constexpr int kMaxCatchUpPhysicsStepsPerServerTick = 192;
/// Drop excess **wall-clock** backlog so a long stall does not freeze the process (seconds).
inline constexpr double kMaxWallPhysicsDebtSeconds = 0.5;

/// MP client sim wall pace vs `(target - head)` (physics steps, signed): on time → 1; up to
/// `kMpLeadCapSteps` **ahead** (`head > target`) → `1 - ratio²` down to 0; up to `kMpLeadCapSteps`
/// **behind** → `1 + ratio² * (max-1)` up to `kMpClientPaceScaleMax`.
inline constexpr double kMpClientPaceScaleMax = 4.0;

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

/// --- Client stress logging (main thread; `[mp-stress]` only when unhealthy) ---

/// Render frame hitch: likely main-thread backlog for net + sim queues.
inline constexpr double kMpClientStressFrameHitchSeconds = 0.060;
/// Confirmed authority step this far ahead of integrated physics head (cannot keep up).
inline constexpr std::uint64_t kMpClientStressBehindAuthoritySteps = 200;
/// Server `ShipState` step this far ahead of local integrated head (stale prediction vs server).
inline constexpr std::uint64_t kMpClientStressShipStateAheadSteps = 200;
/// Prediction buffer within this many steps of the hard lead-vs-target cap (integration
/// stalling).
inline constexpr std::uint64_t kMpClientStressLeadNearCapSlackSteps = 40;
/// Sim thread snapshot FIFO depth (main thread not dequeuing fast enough).
inline constexpr std::size_t kMpClientStressSnapshotJobQueueDepth = 32;
/// Minimum real time between `[mp-stress]` lines.
inline constexpr double kMpClientStressLogCooldownSeconds = 1.75;

}  // namespace net
