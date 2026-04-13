#pragma once

#include "sim/CommandQueue.hpp"
#include "sim/SimulationConfig.hpp"

#include <SFML/System/Vector2.hpp>

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace io {

struct PersistedUiState {
	bool trailsEnabled = false;
	bool relativeTrails = false;
	bool predictionsEnabled = true;
	bool followSelected = false;
	bool creationRelativeFrame = false;
	bool negativeMass = false;
	double creationDensity = 1.0e6;
	int predictionSteps = 220;
	double predictionDt = 1.0 / 120.0;
	std::size_t presetIndex = 0;
};

struct PersistedCameraState {
	sf::Vector2f center{0.0f, 0.0f};
	sf::Vector2f size{1400.0f, 900.0f};
};

struct PersistedWorldState {
	sim::SimulationConfig simConfig;
	double simulationTimeSeconds = 0.0;
	PersistedUiState ui;
	PersistedCameraState camera;
	std::vector<sim::SpawnCommand> bodies;
};

class Persistence {
   public:
	static bool save(const std::string& path,
	                 const PersistedWorldState& world,
	                 std::string& errorOut);
	static bool load(const std::string& path, PersistedWorldState& worldOut, std::string& errorOut);
};

}  // namespace io
