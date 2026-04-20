#include "scenario/ScenarioManager.hpp"
#include "sim/SimulationEngine.hpp"

#include <chrono>
#include <iostream>
#include <thread>
#include <vector>

int main() {
	sim::SimulationConfig cfg;
	cfg.timeScale = 3600.0;
	cfg.fixedDtSeconds = 1.0 / 240.0;
	cfg.gravitationalConstant = 6.67430e-11;
	cfg.softeningEpsilon = 1.0e6;
	cfg.barnesHutTheta = 0.7;
	cfg.collisionCellScale = 8.0;
	cfg.workerCount = 0;

	sim::SimulationEngine engine(cfg);
	engine.start();

	const std::vector<sim::SpawnCommand> world =
	    scenario::ScenarioManager::makeRandom(20000, 0.0, 0.0, 3.5e12);
	engine.queueReplaceWorld(world);

	const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
	std::size_t count = 0;
	while (std::chrono::steady_clock::now() < deadline) {
		count = engine.bodyCount();
		if (count >= 10000) {
			break;
		}
		std::this_thread::sleep_for(std::chrono::milliseconds(20));
	}

	engine.stop();

	if (count < 10000) {
		std::cerr << "[FAIL] high-load smoke: body publication too low: " << count << '\n';
		return 1;
	}

	std::cout << "[PASS] high-load smoke, published bodies=" << count << '\n';
	return 0;
}
