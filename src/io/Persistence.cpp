#include "io/Persistence.hpp"

#include <cmath>
#include <cstddef>
#include <cstdint>
#include <fstream>
#include <limits>
#include <string>

namespace io {

namespace {

constexpr std::uint32_t kMagic = 0x5047534D;  // PGSM
constexpr std::uint32_t kVersion = 1;
constexpr std::uint32_t kMaxBodies = 1'000'000;

template <typename T>
bool writeRaw(std::ofstream& out, const T& value) {
	out.write(reinterpret_cast<const char*>(&value), sizeof(T));
	return out.good();
}

template <typename T>
bool readRaw(std::ifstream& in, T& value) {
	in.read(reinterpret_cast<char*>(&value), sizeof(T));
	return in.good();
}

bool finiteOrFail(double value) {
	return std::isfinite(value);
}

}  // namespace

bool Persistence::save(const std::string& path,
                       const PersistedWorldState& world,
                       std::string& errorOut) {
	std::ofstream out(path, std::ios::binary | std::ios::trunc);
	if (!out.is_open()) {
		errorOut = "Failed to open file for writing";
		return false;
	}

	if (!writeRaw(out, kMagic) || !writeRaw(out, kVersion)) {
		errorOut = "Failed writing file header";
		return false;
	}

	const std::uint32_t bodyCount = static_cast<std::uint32_t>(world.bodies.size());
	if (!writeRaw(out, bodyCount)) {
		errorOut = "Failed writing body count";
		return false;
	}

	const sim::SimulationConfig& cfg = world.simConfig;
	const std::uint32_t mode = static_cast<std::uint32_t>(cfg.mode);
	const std::uint8_t paused = cfg.paused ? 1u : 0u;
	if (!writeRaw(out, mode) || !writeRaw(out, paused) || !writeRaw(out, cfg.timeScale) ||
	    !writeRaw(out, cfg.fixedDtSeconds) || !writeRaw(out, cfg.realtimeMinDtSeconds) ||
	    !writeRaw(out, cfg.realtimeMaxDtSeconds) || !writeRaw(out, cfg.gravitationalConstant) ||
	    !writeRaw(out, cfg.softeningEpsilon) || !writeRaw(out, cfg.barnesHutTheta) ||
	    !writeRaw(out, cfg.collisionCellScale) || !writeRaw(out, cfg.workerCount) ||
	    !writeRaw(out, world.simulationTimeSeconds)) {
		errorOut = "Failed writing simulation metadata";
		return false;
	}

	const PersistedUiState& ui = world.ui;
	const std::uint8_t uiFlags[] = {
	    static_cast<std::uint8_t>(ui.trailsEnabled),
	    static_cast<std::uint8_t>(ui.relativeTrails),
	    static_cast<std::uint8_t>(ui.predictionsEnabled),
	    static_cast<std::uint8_t>(ui.followSelected),
	    static_cast<std::uint8_t>(ui.creationRelativeFrame),
	    static_cast<std::uint8_t>(ui.negativeMass),
	};
	for (const std::uint8_t flag : uiFlags) {
		if (!writeRaw(out, flag)) {
			errorOut = "Failed writing UI flags";
			return false;
		}
	}
	if (!writeRaw(out, ui.creationDensity) || !writeRaw(out, ui.predictionSteps) ||
	    !writeRaw(out, ui.predictionDt) || !writeRaw(out, ui.presetIndex) ||
	    !writeRaw(out, world.camera.center.x) || !writeRaw(out, world.camera.center.y) ||
	    !writeRaw(out, world.camera.size.x) || !writeRaw(out, world.camera.size.y)) {
		errorOut = "Failed writing UI/camera metadata";
		return false;
	}

	for (const sim::SpawnCommand& body : world.bodies) {
		if (!writeRaw(out, body.x) || !writeRaw(out, body.y) || !writeRaw(out, body.vx) ||
		    !writeRaw(out, body.vy) || !writeRaw(out, body.mass) || !writeRaw(out, body.radius)) {
			errorOut = "Failed writing body data";
			return false;
		}
	}

	return true;
}

bool Persistence::load(const std::string& path,
                       PersistedWorldState& worldOut,
                       std::string& errorOut) {
	std::ifstream in(path, std::ios::binary);
	if (!in.is_open()) {
		errorOut = "Failed to open file for reading";
		return false;
	}

	std::uint32_t magic = 0;
	std::uint32_t version = 0;
	if (!readRaw(in, magic) || !readRaw(in, version)) {
		errorOut = "Failed reading header";
		return false;
	}
	if (magic != kMagic) {
		errorOut = "Invalid file magic";
		return false;
	}
	if (version != kVersion) {
		errorOut = "Unsupported file version";
		return false;
	}

	std::uint32_t bodyCount = 0;
	if (!readRaw(in, bodyCount)) {
		errorOut = "Failed reading body count";
		return false;
	}
	if (bodyCount > kMaxBodies) {
		errorOut = "Body count exceeds supported limit";
		return false;
	}

	PersistedWorldState loaded{};
	std::uint32_t mode = 0;
	std::uint8_t paused = 0;
	if (!readRaw(in, mode) || !readRaw(in, paused) || !readRaw(in, loaded.simConfig.timeScale) ||
	    !readRaw(in, loaded.simConfig.fixedDtSeconds) ||
	    !readRaw(in, loaded.simConfig.realtimeMinDtSeconds) ||
	    !readRaw(in, loaded.simConfig.realtimeMaxDtSeconds) ||
	    !readRaw(in, loaded.simConfig.gravitationalConstant) ||
	    !readRaw(in, loaded.simConfig.softeningEpsilon) ||
	    !readRaw(in, loaded.simConfig.barnesHutTheta) ||
	    !readRaw(in, loaded.simConfig.collisionCellScale) ||
	    !readRaw(in, loaded.simConfig.workerCount) || !readRaw(in, loaded.simulationTimeSeconds)) {
		errorOut = "Failed reading simulation metadata";
		return false;
	}
	loaded.simConfig.mode = static_cast<sim::SimulationMode>(mode);
	loaded.simConfig.paused = paused != 0;

	std::uint8_t uiFlags[6]{};
	for (std::uint8_t& flag : uiFlags) {
		if (!readRaw(in, flag)) {
			errorOut = "Failed reading UI flags";
			return false;
		}
	}
	loaded.ui.trailsEnabled = uiFlags[0] != 0;
	loaded.ui.relativeTrails = uiFlags[1] != 0;
	loaded.ui.predictionsEnabled = uiFlags[2] != 0;
	loaded.ui.followSelected = uiFlags[3] != 0;
	loaded.ui.creationRelativeFrame = uiFlags[4] != 0;
	loaded.ui.negativeMass = uiFlags[5] != 0;

	if (!readRaw(in, loaded.ui.creationDensity) || !readRaw(in, loaded.ui.predictionSteps) ||
	    !readRaw(in, loaded.ui.predictionDt) || !readRaw(in, loaded.ui.presetIndex) ||
	    !readRaw(in, loaded.camera.center.x) || !readRaw(in, loaded.camera.center.y) ||
	    !readRaw(in, loaded.camera.size.x) || !readRaw(in, loaded.camera.size.y)) {
		errorOut = "Failed reading UI/camera metadata";
		return false;
	}

	loaded.bodies.resize(bodyCount);
	for (std::uint32_t i = 0; i < bodyCount; ++i) {
		sim::SpawnCommand body{};
		if (!readRaw(in, body.x) || !readRaw(in, body.y) || !readRaw(in, body.vx) ||
		    !readRaw(in, body.vy) || !readRaw(in, body.mass) || !readRaw(in, body.radius)) {
			errorOut = "Failed reading body data";
			return false;
		}
		if (!finiteOrFail(body.x) || !finiteOrFail(body.y) || !finiteOrFail(body.vx) ||
		    !finiteOrFail(body.vy) || !finiteOrFail(body.mass) || !finiteOrFail(body.radius)) {
			errorOut = "Body contains non-finite values";
			return false;
		}
		if (std::abs(body.mass) < 1e-12 || body.radius <= 0.0) {
			errorOut = "Body contains invalid mass/radius";
			return false;
		}
		loaded.bodies[i] = body;
	}

	if (!finiteOrFail(loaded.simulationTimeSeconds) || !finiteOrFail(loaded.simConfig.timeScale) ||
	    !finiteOrFail(loaded.simConfig.fixedDtSeconds) ||
	    !finiteOrFail(loaded.simConfig.realtimeMinDtSeconds) ||
	    !finiteOrFail(loaded.simConfig.realtimeMaxDtSeconds) ||
	    !finiteOrFail(loaded.simConfig.gravitationalConstant) ||
	    !finiteOrFail(loaded.simConfig.softeningEpsilon) ||
	    !finiteOrFail(loaded.simConfig.barnesHutTheta) ||
	    !finiteOrFail(loaded.simConfig.collisionCellScale) || loaded.ui.predictionSteps < 1 ||
	    !finiteOrFail(loaded.ui.creationDensity) || !finiteOrFail(loaded.ui.predictionDt)) {
		errorOut = "Metadata validation failed";
		return false;
	}

	worldOut = std::move(loaded);
	return true;
}

}  // namespace io
