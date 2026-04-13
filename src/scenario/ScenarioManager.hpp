#pragma once

#include "sim/CommandQueue.hpp"

#include <cstddef>
#include <string>
#include <vector>

namespace scenario {

class ScenarioManager {
   public:
	[[nodiscard]] static std::vector<std::string> presetNames();
	[[nodiscard]] static std::vector<sim::SpawnCommand> makePreset(std::size_t index);
	[[nodiscard]] static std::vector<sim::SpawnCommand> makeRandom(std::size_t count,
	                                                               double centerX,
	                                                               double centerY,
	                                                               double spreadRadius);
};

}  // namespace scenario
