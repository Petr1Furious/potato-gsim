#pragma once

#include "sim/ChunkPolicy.hpp"

#include <cstddef>

namespace sim {

struct SimulationConfig {
	/// Wall-clock seconds × timeScale = simulated seconds per integration step (simple clamp).
	double timeScale = 1.0;
	double fixedDtSeconds = 1.0 / 120.0;

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
