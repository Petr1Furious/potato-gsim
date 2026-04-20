#include "sim/SimulationEngine.hpp"

#include <chrono>
#include <cmath>
#include <functional>
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

std::optional<sim::BodySnapshot> waitForSingleBody(
    sim::SimulationEngine& engine,
    const std::function<bool(const sim::BodySnapshot&)>& predicate) {
	const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
	std::vector<sim::BodySnapshot> bodies;
	while (std::chrono::steady_clock::now() < deadline) {
		engine.copyBodies(bodies);
		if (bodies.size() == 1 && predicate(bodies.front())) {
			return bodies.front();
		}
		std::this_thread::sleep_for(std::chrono::milliseconds(10));
	}
	return std::nullopt;
}

bool testBoundarySwitching() {
	sim::SimulationConfig cfg;
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

bool testMergeSurvivorSelection() {
	sim::SimulationConfig cfg;
	cfg.timeScale = 0.01;
	cfg.gravitationalConstant = 0.0;
	cfg.collisionStepInterval = 1;
	cfg.workerCount = 0;
	cfg.directSerialMaxBodies = 64;
	cfg.directParallelMaxBodies = 64;

	sim::SimulationEngine engine(cfg);
	engine.start();
	bool ok = true;

	engine.queueReplaceWorld({
	    sim::SpawnCommand{.x = 0.0, .y = 0.0, .vx = 0.0, .vy = 0.0, .mass = 5.0, .radius = 2.0},
	    sim::SpawnCommand{.x = 0.5, .y = 0.0, .vx = 0.0, .vy = 0.0, .mass = -9.0, .radius = 2.0},
	});
	const std::optional<sim::BodySnapshot> heavyWinner = waitForSingleBody(
	    engine, [](const sim::BodySnapshot& body) { return std::abs(body.mass + 4.0) < 1e-9; });
	ok &= expect(heavyWinner.has_value(), "Expected first overlap to merge into one body");
	if (heavyWinner.has_value()) {
		ok &= expect(sim::bodyIdSlot(heavyWinner->id) == 1,
		             "Merge survivor should prefer larger abs(mass)");
	}

	engine.queueReplaceWorld({
	    sim::SpawnCommand{.x = 0.0, .y = 0.0, .vx = 0.0, .vy = 0.0, .mass = 8.0, .radius = 2.0},
	    sim::SpawnCommand{.x = 0.5, .y = 0.0, .vx = 0.0, .vy = 0.0, .mass = 8.0, .radius = 2.0},
	});
	const std::optional<sim::BodySnapshot> tieWinner = waitForSingleBody(
	    engine, [](const sim::BodySnapshot& body) { return std::abs(body.mass - 16.0) < 1e-9; });
	ok &= expect(tieWinner.has_value(), "Expected tie overlap to merge into one body");
	if (tieWinner.has_value()) {
		ok &= expect(sim::bodyIdSlot(tieWinner->id) == 0,
		             "Merge survivor tie-break should prefer lower BodyId");
	}

	engine.stop();
	return ok;
}

}  // namespace

int main() {
	bool ok = true;
	ok &= testBoundarySwitching();
	ok &= testThresholdClamping();
	ok &= testMergeSurvivorSelection();
	if (!ok) {
		return 1;
	}
	std::cout << "[PASS] algorithm switching tests\n";
	return 0;
}
