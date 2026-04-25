#pragma once

#include "sim/CommandQueue.hpp"

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace scenario {

enum class PresetKind : std::uint8_t {
	SolarLike = 0,
	BinaryDance = 1,
	SpiralCluster = 2,
	Random = 3,
};

struct RandomPresetConfig {
	std::size_t count = 1000;
	double centerX = 0.0;
	double centerY = 0.0;
	double spreadRadius = 1e11;
	double massMin = 1e21;
	double massMax = 5e25;
	double jitter = 1.5e9;
	/// 0 keeps random bodies static; >0 scales tangential speed around center.
	double tangentialVelocityScale = 0.0;
	std::uint64_t seed = 0;
	bool useDeterministicSeed = false;
};

class ScenarioManager {
   public:
	[[nodiscard]] static std::vector<std::string> presetNames();
	[[nodiscard]] static PresetKind presetKindFromIndex(std::size_t index);
	[[nodiscard]] static std::vector<sim::SpawnCommand> makePreset(std::size_t index);
	[[nodiscard]] static std::vector<sim::SpawnCommand> makePreset(PresetKind preset);
	[[nodiscard]] static std::vector<sim::SpawnCommand> makeRandom(std::size_t count,
	                                                               double centerX,
	                                                               double centerY,
	                                                               double spreadRadius);
	[[nodiscard]] static std::vector<sim::SpawnCommand> makeRandom(const RandomPresetConfig& cfg);
};

}  // namespace scenario
