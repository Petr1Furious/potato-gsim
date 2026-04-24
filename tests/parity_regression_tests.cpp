#include "io/Persistence.hpp"
#include "scenario/ScenarioManager.hpp"
#include "sim/SimulationConfig.hpp"
#include "ui/CreationTool.hpp"
#include "ui/Predictor.hpp"

#include <SFML/System/Vector2.hpp>

#include <cmath>
#include <filesystem>
#include <iostream>
#include <optional>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const std::string& message) {
	if (!condition) {
		std::cerr << "[FAIL] " << message << '\n';
		return false;
	}
	return true;
}

bool testPhysicalPresetScale() {
	const std::vector<sim::SpawnCommand> solar = scenario::ScenarioManager::makePreset(0);
	bool ok = true;
	ok &= expect(!solar.empty(), "Solar preset should not be empty");
	ok &= expect(solar.front().mass > 1e29, "Solar preset central mass should be star-scale");
	ok &= expect(solar.front().radius > 1e8, "Solar preset central radius should be star-scale");
	if (solar.size() > 3) {
		ok &= expect(solar[3].x > 1e11, "Earth-like orbit distance should be astronomical scale");
		ok &= expect(std::abs(solar[3].vy) > 1e4, "Earth-like orbit speed should be km/s scale");
	} else {
		ok &= expect(false, "Solar preset should include planets");
	}
	return ok;
}

bool testCreationCancelAndDensityClamp() {
	ui::CreationTool tool;
	bool ok = true;
	tool.setEnabled(true);
	tool.setDensity(10.0);
	ok &= expect(std::abs(tool.density() - 100.0) < 1e-9,
	             "Creation density should clamp to lower bound");
	tool.setDensity(90000.0);
	ok &= expect(std::abs(tool.density() - 30000.0) < 1e-9,
	             "Creation density should clamp to upper bound");

	const sf::Vector2f p0(0.0f, 0.0f);
	const sf::Vector2f p1(100.0f, 0.0f);
	const sf::Vector2f p2(100.0f, 100.0f);
	std::optional<sim::SpawnCommand> spawn = tool.handleLeftClick(p0, std::nullopt, 1.0);
	ok &= expect(!spawn.has_value(), "Creation step 1 should not spawn");
	spawn = tool.handleLeftClick(p1, std::nullopt, 1.0);
	ok &= expect(!spawn.has_value(), "Creation step 2 should not spawn");
	tool.cancel();
	ok &= expect(tool.step() == ui::CreationTool::Step::Inactive,
	             "Creation cancel should return to inactive step");
	spawn = tool.handleLeftClick(p2, std::nullopt, 1.0);
	ok &= expect(!spawn.has_value(), "After cancel, first click should restart tool");
	return ok;
}

bool testPersistenceRoundTrip() {
	io::PersistedWorldState world;
	world.simConfig.timeScale = 86400.0;
	world.simConfig.gravitationalConstant = 6.67430e-11;
	world.simConfig.softeningEpsilon = 1.0e6;
	world.simulationTimeSeconds = 12345.0;
	world.ui.creationDensity = 5514.0;
	world.ui.negativeMass = true;
	world.ui.alwaysShowNames = true;
	world.ui.predictionRecalcIntervalSeconds = 0.04;
	world.camera.center = sf::Vector2f(1e11f, -2e11f);
	world.camera.size = sf::Vector2f(5e12f, 3e12f);
	world.bodies = {
	    sim::SpawnCommand{
	        .x = 0.0,
	        .y = 0.0,
	        .vx = 0.0,
	        .vy = 0.0,
	        .mass = 1.98847e30,
	        .radius = 6.9634e8,
	        .name = "Sun",
	    },
	    sim::SpawnCommand{
	        .x = 1.496e11,
	        .y = 0.0,
	        .vx = 0.0,
	        .vy = 2.978e4,
	        .mass = 5.972e24,
	        .radius = 6.371e6,
	        .name = "Earth",
	    },
	};

	const std::filesystem::path path =
	    std::filesystem::temp_directory_path() / "potato_gsim_parity_roundtrip.pgsim";
	std::string error;
	bool ok = expect(io::Persistence::save(path.string(), world, error),
	                 "Persistence save should succeed");
	io::PersistedWorldState loaded;
	ok &= expect(io::Persistence::load(path.string(), loaded, error),
	             "Persistence load should succeed");
	if (ok) {
		ok &= expect(loaded.bodies.size() == world.bodies.size(), "Loaded body count should match");
		ok &= expect(std::abs(loaded.simConfig.gravitationalConstant - 6.67430e-11) < 1e-18,
		             "Loaded gravity constant should keep SI value");
		ok &= expect(std::abs(loaded.ui.creationDensity - 5514.0) < 1e-6,
		             "Loaded creation density should round-trip");
		ok &= expect(loaded.ui.negativeMass, "Loaded negative mass flag should round-trip");
		ok &= expect(loaded.ui.alwaysShowNames, "Loaded always-show-names should round-trip");
		ok &= expect(std::abs(loaded.ui.predictionRecalcIntervalSeconds - 0.04) < 1e-9,
		             "Loaded prediction recalc interval should round-trip");
		ok &= expect(loaded.bodies[0].name == "Sun" && loaded.bodies[1].name == "Earth",
		             "Loaded body names should round-trip");
	}
	std::error_code ignore;
	std::filesystem::remove(path, ignore);
	return ok;
}

bool testRandomScenarioMassiveOnly() {
	const std::vector<sim::SpawnCommand> random =
	    scenario::ScenarioManager::makeRandom(1200, 0.0, 0.0, 2.5e12);
	bool ok = true;
	ok &= expect(!random.empty(), "Random scenario should not be empty");
	for (const sim::SpawnCommand& body : random) {
		if (!(body.mass > 0.0)) {
			ok &= expect(false, "Random scenario should not include non-positive masses");
			break;
		}
		if (!(body.radius > 0.0)) {
			ok &= expect(false, "Random scenario should not include non-positive radii");
			break;
		}
	}
	return ok;
}

bool testPredictorCollisionMerge() {
	ui::Predictor predictor;
	ui::Predictor::Settings settings = predictor.settings();
	settings.steps = 12;
	settings.dt = 0.1;
	settings.maxAttractors = 16;
	predictor.setSettings(settings);

	const std::vector<sim::BodySnapshot> bodies{
	    sim::BodySnapshot{
	        .id = 1,
	        .x = -1.0,
	        .y = 0.0,
	        .vx = 0.0,
	        .vy = 0.0,
	        .mass = 1.0,
	        .radius = 2.0,
	    },
	    sim::BodySnapshot{
	        .id = 2,
	        .x = 1.0,
	        .y = 0.0,
	        .vx = 0.0,
	        .vy = 0.0,
	        .mass = 5.0,
	        .radius = 2.0,
	    },
	};
	const ui::PredictionPath predicted = predictor.predictForBody(bodies, 1, 0.0, 1e-6);
	bool ok = true;
	ok &= expect(!predicted.points.empty(), "Predictor should return points for merged body");
	ok &= expect(!predicted.stoppedOnEncounter,
	             "Initially overlapping bodies use merge path, not encounter stop");
	if (!predicted.points.empty()) {
		const double expectedX = ((-1.0 * 1.0) + (1.0 * 5.0)) / 6.0;
		ok &= expect(std::abs(static_cast<double>(predicted.points.front().x) - expectedX) < 1e-4,
		             "Predictor should remap tracked id through merge and start at COM");
	}
	return ok;
}

}  // namespace

int main() {
	bool ok = true;
	ok &= testPhysicalPresetScale();
	ok &= testCreationCancelAndDensityClamp();
	ok &= testPersistenceRoundTrip();
	ok &= testRandomScenarioMassiveOnly();
	ok &= testPredictorCollisionMerge();

	if (!ok) {
		return 1;
	}
	std::cout << "[PASS] parity regression tests\n";
	return 0;
}
