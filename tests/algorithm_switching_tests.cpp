#include "sim/SimulationEngine.hpp"

#include <chrono>
#include <iostream>
#include <string>
#include <thread>
#include <vector>

namespace {

bool expect(bool condition, const std::string& message) {
	if (!condition) {
		std::cerr << "[FAIL] " << message << '\n';
		return false;
	}
	return true;
}

std::vector<sim::SpawnCommand> makeWorld(std::size_t count) {
	std::vector<sim::SpawnCommand> out;
	out.reserve(count);
	for (std::size_t i = 0; i < count; ++i) {
		out.push_back(sim::SpawnCommand{
		    .x = static_cast<double>(i) * 1.0e9,
		    .y = (i & 1u) == 0u ? 0.0 : 5.0e8,
		    .vx = 0.0,
		    .vy = 0.0,
		    .mass = 1.0e12,
		    .radius = 1.0,
		});
	}
	return out;
}

bool waitForBodyCount(sim::SimulationEngine& engine, std::size_t expected) {
	const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
	while (std::chrono::steady_clock::now() < deadline) {
		if (engine.bodyCount() == expected) {
			return true;
		}
		std::this_thread::sleep_for(std::chrono::milliseconds(10));
	}
	return false;
}

bool waitForAlgorithm(sim::SimulationEngine& engine,
                      std::size_t expectedBodies,
                      const std::string& expectedAlgorithm) {
	const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
	while (std::chrono::steady_clock::now() < deadline) {
		const sim::EngineDebugStats stats = engine.debugStats();
		if (stats.valid && stats.bodyCount == expectedBodies &&
		    std::string(stats.forceAlgorithm) == expectedAlgorithm) {
			return true;
		}
		std::this_thread::sleep_for(std::chrono::milliseconds(10));
	}
	return false;
}

bool testBoundarySwitching() {
	sim::SimulationConfig cfg;
	cfg.mode = sim::SimulationMode::DeterministicFixedStep;
	cfg.timeScale = 1.0;
	cfg.gravitationalConstant = 0.0;
	cfg.collisionStepInterval = 8;
	cfg.workerCount = 1;
	cfg.directSerialMaxBodies = 64;
	cfg.directParallelMaxBodies = 2048;

	sim::SimulationEngine engine(cfg);
	engine.start();

	struct Case {
		std::size_t n;
		const char* algorithm;
	};
	const std::vector<Case> cases{
	    {2, "direct_serial"},    {9, "direct_serial"},      {64, "direct_serial"},
	    {65, "direct_parallel"}, {2048, "direct_parallel"}, {2049, "barnes_hut_parallel"},
	};

	bool ok = true;
	for (const Case c : cases) {
		engine.queueReplaceWorld(makeWorld(c.n));
		ok &= expect(waitForBodyCount(engine, c.n),
		             "Timed out waiting for body count " + std::to_string(c.n));
		ok &= expect(waitForAlgorithm(engine, c.n, c.algorithm),
		             "Algorithm mismatch at N=" + std::to_string(c.n) + " expected " + c.algorithm);
	}

	engine.stop();
	return ok;
}

bool testThresholdClamping() {
	sim::SimulationEngine engine;
	engine.setDirectSerialMaxBodies(4096);
	engine.setDirectParallelMaxBodies(2048);
	const sim::SimulationConfig cfg = engine.config();
	return expect(cfg.directSerialMaxBodies <= cfg.directParallelMaxBodies,
	              "direct_serial threshold must stay <= direct_parallel threshold");
}

}  // namespace

int main() {
	bool ok = true;
	ok &= testBoundarySwitching();
	ok &= testThresholdClamping();
	if (!ok) {
		return 1;
	}
	std::cout << "[PASS] algorithm switching tests\n";
	return 0;
}
