#pragma once

#include <algorithm>
#include <cstddef>

namespace sim {

enum class SimulationMode { DeterministicFixedStep, RealTimeVariableStep };

struct SimulationConfig {
	SimulationMode mode = SimulationMode::DeterministicFixedStep;
	double timeScale = 1.0;

	// Deterministic mode
	double fixedDtSeconds = 1.0 / 120.0;

	// Real-time mode (simulation-clock dt clamping)
	double realtimeMinDtSeconds = 1.0 / 240.0;
	double realtimeMaxDtSeconds = 1.0 / 30.0;

	// Gravity and Barnes-Hut controls
	double gravitationalConstant = 6.67430e-11;
	double softeningEpsilon = 1.0;
	double barnesHutTheta = 0.6;

	// Collision grid
	double collisionCellScale = 4.0;

	// Parallelism
	std::size_t workerCount = 0;

	bool paused = false;

	[[nodiscard]] double clampRealtimeDt(double dt) const {
		return std::clamp(dt, realtimeMinDtSeconds, realtimeMaxDtSeconds);
	}
};

}  // namespace sim
