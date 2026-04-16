#pragma once

#include "sim/ChunkPolicy.hpp"

#include <cstddef>

namespace sim {

enum class SimulationMode { DeterministicFixedStep, RealTimeVariableStep };

struct SimulationConfig {
	SimulationMode mode = SimulationMode::DeterministicFixedStep;
	double timeScale = 1.0;

	// Deterministic mode
	double fixedDtSeconds = 1.0 / 120.0;

	// Real-time mode (wall-dt outlier clamping)
	std::size_t realtimeDtWindowSize = 120;
	double realtimeDtSpikeClampMultiplier = 6.0;
	std::size_t realtimeDtClampWarmupSamples = 16;

	// Gravity and Barnes-Hut controls
	double gravitationalConstant = 6.67430e-11;
	double softeningEpsilon = 1.0;
	double barnesHutTheta = 0.6;

	// Collision grid
	double collisionCellScale = 8.0;
	std::size_t collisionStepInterval = 2;

	// Parallelism
	std::size_t workerCount = 0;
	std::size_t parallelChunkSize = 0;  // 0 = auto
	ChunkPolicy chunkPolicy = ChunkPolicy::DynamicClaim;

	// Algorithm switching thresholds
	std::size_t directSerialMaxBodies = 224;
	std::size_t directParallelMaxBodies = 768;

	bool paused = false;
};

}  // namespace sim
