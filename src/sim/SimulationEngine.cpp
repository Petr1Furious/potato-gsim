#include "sim/SimulationEngine.hpp"
#include "sim/UnionFind.hpp"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <numbers>
#include <random>
#include <thread>

namespace sim {

namespace {

[[nodiscard]] double clampPositive(double value, double fallback) {
	if (std::isfinite(value) && value > 0.0) {
		return value;
	}
	return fallback;
}

[[nodiscard]] double clampFinite(double value, double fallback) {
	if (std::isfinite(value)) {
		return value;
	}
	return fallback;
}

}  // namespace

SimulationEngine::SimulationEngine(SimulationConfig config) : config_(config) {
	states_[0].reserve(1024);
	states_[1].reserve(1024);
}

SimulationEngine::~SimulationEngine() {
	stop();
}

void SimulationEngine::start() {
	std::lock_guard<std::mutex> lock(lifecycleMutex_);
	if (running_.load(std::memory_order_acquire)) {
		return;
	}
	running_.store(true, std::memory_order_release);

	const SimulationConfig cfg = currentConfig();
	threadPool_.resize(desiredWorkers(cfg));
	simulationThread_ =
	    std::jthread([this](std::stop_token stopToken) { simulationLoop(stopToken); });
}

void SimulationEngine::stop() {
	std::lock_guard<std::mutex> lock(lifecycleMutex_);
	if (!running_.load(std::memory_order_acquire)) {
		return;
	}
	running_.store(false, std::memory_order_release);
	if (simulationThread_.joinable()) {
		simulationThread_.request_stop();
		simulationThread_.join();
	}
}

void SimulationEngine::queueSpawn(const SpawnCommand& command) {
	commandQueue_.pushSpawn(command);
}

void SimulationEngine::queueDelete(BodyId id) {
	commandQueue_.pushDelete(id);
}

void SimulationEngine::queueDeleteNearest(double x, double y, double maxDistance) {
	const std::optional<BodyId> id = findNearestBody(x, y, maxDistance);
	if (id.has_value()) {
		queueDelete(*id);
	}
}

void SimulationEngine::queueClearAll() {
	commandQueue_.pushClearAll();
}

void SimulationEngine::queueReplaceWorld(const std::vector<SpawnCommand>& bodies) {
	commandQueue_.pushReplaceWorld(bodies);
}

std::optional<BodyId> SimulationEngine::findNearestBody(double x,
                                                        double y,
                                                        double maxDistance) const {
	std::shared_lock<std::shared_mutex> lock(stateMutex_);
	const BodyState& state = states_[readIndex_];
	const IdIndexMap& idMap = idMaps_[readIndex_];
	if (state.empty()) {
		return std::nullopt;
	}

	const double maxDist2 = maxDistance * maxDistance;
	double bestDist2 = maxDist2;
	std::optional<std::uint32_t> bestIndex;
	for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(state.size()); ++i) {
		const double dx = state.posX[i] - x;
		const double dy = state.posY[i] - y;
		const double d2 = dx * dx + dy * dy;
		if (d2 < bestDist2) {
			bestDist2 = d2;
			bestIndex = i;
		}
	}
	if (!bestIndex.has_value()) {
		return std::nullopt;
	}
	return idMap.idAtDenseIndex(*bestIndex);
}

std::size_t SimulationEngine::bodyCount() const {
	std::shared_lock<std::shared_mutex> lock(stateMutex_);
	return states_[readIndex_].size();
}

void SimulationEngine::withReadState(const std::function<void(const BodyState&)>& fn) const {
	std::shared_lock<std::shared_mutex> lock(stateMutex_);
	fn(states_[readIndex_]);
}

void SimulationEngine::withReadSnapshot(
    const std::function<void(const BodyState&, const IdIndexMap&)>& fn) const {
	std::shared_lock<std::shared_mutex> lock(stateMutex_);
	fn(states_[readIndex_], idMaps_[readIndex_]);
}

void SimulationEngine::copyBodies(std::vector<BodySnapshot>& out) const {
	std::shared_lock<std::shared_mutex> lock(stateMutex_);
	const BodyState& state = states_[readIndex_];
	const IdIndexMap& idMap = idMaps_[readIndex_];
	out.clear();
	out.reserve(state.size());
	for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(state.size()); ++i) {
		out.push_back(BodySnapshot{
		    .id = idMap.idAtDenseIndex(i),
		    .x = state.posX[i],
		    .y = state.posY[i],
		    .vx = state.velX[i],
		    .vy = state.velY[i],
		    .mass = state.mass[i],
		    .radius = state.radius[i],
		});
	}
}

std::optional<BodySnapshot> SimulationEngine::bodyById(BodyId id) const {
	std::shared_lock<std::shared_mutex> lock(stateMutex_);
	const BodyState& state = states_[readIndex_];
	const IdIndexMap& idMap = idMaps_[readIndex_];
	const std::optional<std::uint32_t> idx = idMap.denseIndexFor(id);
	if (!idx.has_value()) {
		return std::nullopt;
	}
	const std::uint32_t i = *idx;
	return BodySnapshot{
	    .id = id,
	    .x = state.posX[i],
	    .y = state.posY[i],
	    .vx = state.velX[i],
	    .vy = state.velY[i],
	    .mass = state.mass[i],
	    .radius = state.radius[i],
	};
}

SimulationConfig SimulationEngine::config() const {
	return currentConfig();
}

double SimulationEngine::simulationTimeSeconds() const {
	std::lock_guard<std::mutex> lock(timerMutex_);
	return simulatedSeconds_;
}

double SimulationEngine::updatesPerSecond() const {
	return updatesPerSecond_.load(std::memory_order_relaxed);
}

double SimulationEngine::simulatedSecondsPerRealSecond() const {
	return simulatedSecondsPerRealSecond_.load(std::memory_order_relaxed);
}

double SimulationEngine::simulatedSecondsPerUpdate() const {
	return simulatedSecondsPerUpdate_.load(std::memory_order_relaxed);
}

void SimulationEngine::resetSimulationTimer() {
	std::lock_guard<std::mutex> lock(timerMutex_);
	simulatedSeconds_ = 0.0;
}

void SimulationEngine::setSimulationTimer(double seconds) {
	std::lock_guard<std::mutex> lock(timerMutex_);
	simulatedSeconds_ = std::max(0.0, seconds);
}

void SimulationEngine::setMode(SimulationMode mode) {
	updateConfig([mode](SimulationConfig& cfg) { cfg.mode = mode; });
}

void SimulationEngine::toggleMode() {
	updateConfig([](SimulationConfig& cfg) {
		cfg.mode = (cfg.mode == SimulationMode::DeterministicFixedStep)
		               ? SimulationMode::RealTimeVariableStep
		               : SimulationMode::DeterministicFixedStep;
	});
}

void SimulationEngine::setPaused(bool paused) {
	updateConfig([paused](SimulationConfig& cfg) { cfg.paused = paused; });
}

void SimulationEngine::togglePaused() {
	updateConfig([](SimulationConfig& cfg) { cfg.paused = !cfg.paused; });
}

void SimulationEngine::setTimeScale(double timeScale) {
	updateConfig(
	    [timeScale](SimulationConfig& cfg) { cfg.timeScale = std::clamp(timeScale, 1e-5, 1e8); });
}

void SimulationEngine::scaleTimeBy(double factor) {
	updateConfig([factor](SimulationConfig& cfg) {
		cfg.timeScale = std::clamp(cfg.timeScale * factor, 1e-5, 1e8);
	});
}

void SimulationEngine::setFixedDt(double fixedDtSeconds) {
	updateConfig([fixedDtSeconds](SimulationConfig& cfg) {
		cfg.fixedDtSeconds = std::clamp(fixedDtSeconds, 1e-6, 1.0);
	});
}

void SimulationEngine::setRealtimeDtRange(double minDtSeconds, double maxDtSeconds) {
	updateConfig([minDtSeconds, maxDtSeconds](SimulationConfig& cfg) {
		const double mn = std::clamp(minDtSeconds, 1e-6, 1.0);
		const double mx = std::clamp(maxDtSeconds, mn, 2.0);
		cfg.realtimeMinDtSeconds = mn;
		cfg.realtimeMaxDtSeconds = mx;
	});
}

void SimulationEngine::setGravityConstant(double gravitationalConstant) {
	updateConfig([gravitationalConstant](SimulationConfig& cfg) {
		cfg.gravitationalConstant = clampFinite(gravitationalConstant, cfg.gravitationalConstant);
	});
}

void SimulationEngine::setCollisionCellScale(double cellScale) {
	updateConfig([cellScale](SimulationConfig& cfg) {
		cfg.collisionCellScale = std::clamp(cellScale, 0.5, 64.0);
	});
}

void SimulationEngine::setTheta(double theta) {
	updateConfig(
	    [theta](SimulationConfig& cfg) { cfg.barnesHutTheta = std::clamp(theta, 0.1, 1.5); });
}

void SimulationEngine::setSoftening(double epsilon) {
	updateConfig(
	    [epsilon](SimulationConfig& cfg) { cfg.softeningEpsilon = std::max(1e-6, epsilon); });
}

void SimulationEngine::setWorkerCount(std::size_t workers) {
	updateConfig([workers](SimulationConfig& cfg) { cfg.workerCount = workers; });
}

void SimulationEngine::seedCircularCloud(std::size_t count,
                                         double centerX,
                                         double centerY,
                                         double spreadRadius) {
	std::mt19937_64 rng(42);
	std::uniform_real_distribution<double> angleDist(0.0, 2.0 * std::numbers::pi);
	std::uniform_real_distribution<double> radiusDist(0.0, 1.0);

	for (std::size_t i = 0; i < count; ++i) {
		const double angle = angleDist(rng);
		const double r = std::sqrt(radiusDist(rng)) * spreadRadius;
		const double x = centerX + std::cos(angle) * r;
		const double y = centerY + std::sin(angle) * r;

		const double mass = 5e10 + static_cast<double>(i % 100u) * 5e8;
		const double visualRadius = 0.8 + static_cast<double>((i * 7u) % 10u) * 0.08;

		// Tangential starter velocity for visible orbital motion.
		const double tangentSpeed =
		    std::sqrt(std::max(0.0, 2e-3 * spreadRadius / std::max(10.0, r + 10.0)));
		const double vx = -std::sin(angle) * tangentSpeed;
		const double vy = std::cos(angle) * tangentSpeed;

		queueSpawn(SpawnCommand{
		    .x = x,
		    .y = y,
		    .vx = vx,
		    .vy = vy,
		    .mass = mass,
		    .radius = visualRadius,
		});
	}
}

void SimulationEngine::simulationLoop(std::stop_token stopToken) {
	using clock = std::chrono::steady_clock;
	auto lastTick = clock::now();
	auto lastUpsSample = lastTick;
	std::size_t stepsSinceUpsSample = 0;
	double simulatedSecondsSinceSample = 0.0;
	SimulationMode lastMode = currentConfig().mode;

	while (!stopToken.stop_requested()) {
		const SimulationConfig cfg = currentConfig();
		const auto now = clock::now();
		const double elapsed = std::chrono::duration<double>(now - lastTick).count();
		lastTick = now;

		if (cfg.mode != lastMode) {
			lastMode = cfg.mode;
			stepsSinceUpsSample = 0;
			simulatedSecondsSinceSample = 0.0;
			lastUpsSample = now;
			updatesPerSecond_.store(0.0, std::memory_order_relaxed);
			simulatedSecondsPerRealSecond_.store(0.0, std::memory_order_relaxed);
			simulatedSecondsPerUpdate_.store(0.0, std::memory_order_relaxed);
		}

		if (!running_.load(std::memory_order_acquire)) {
			break;
		}

		if (cfg.paused) {
			const double sampleWindow =
			    std::chrono::duration<double>(clock::now() - lastUpsSample).count();
			if (sampleWindow >= 0.25) {
				updatesPerSecond_.store(0.0, std::memory_order_relaxed);
				simulatedSecondsPerRealSecond_.store(0.0, std::memory_order_relaxed);
				simulatedSecondsPerUpdate_.store(0.0, std::memory_order_relaxed);
				stepsSinceUpsSample = 0;
				simulatedSecondsSinceSample = 0.0;
				lastUpsSample = clock::now();
			}
			std::this_thread::sleep_for(std::chrono::milliseconds(1));
			continue;
		}

		if (cfg.mode == SimulationMode::DeterministicFixedStep) {
			// Deterministic mode intentionally runs at maximum update throughput.
			// This keeps step size fixed, but does not gate simulation by wall-clock pacing.
			const double scaledFixedDt = std::max(1e-6, cfg.fixedDtSeconds * cfg.timeScale);
			step(scaledFixedDt, cfg);
			++stepsSinceUpsSample;
			simulatedSecondsSinceSample += scaledFixedDt;
		} else {
			const double elapsedRealtime = std::max(0.0, elapsed);
			const double realtimeDt = std::min(elapsedRealtime, cfg.realtimeMaxDtSeconds);
			if (realtimeDt <= 0.0) {
				continue;
			}
			const double dt = realtimeDt * cfg.timeScale;
			step(dt, cfg);
			++stepsSinceUpsSample;
			simulatedSecondsSinceSample += dt;
		}

		const double sampleWindow =
		    std::chrono::duration<double>(clock::now() - lastUpsSample).count();
		if (sampleWindow >= 0.25) {
			updatesPerSecond_.store(static_cast<double>(stepsSinceUpsSample) / sampleWindow,
			                        std::memory_order_relaxed);
			simulatedSecondsPerRealSecond_.store(simulatedSecondsSinceSample / sampleWindow,
			                                     std::memory_order_relaxed);
			simulatedSecondsPerUpdate_.store(
			    stepsSinceUpsSample > 0
			        ? (simulatedSecondsSinceSample / static_cast<double>(stepsSinceUpsSample))
			        : 0.0,
			    std::memory_order_relaxed);
			stepsSinceUpsSample = 0;
			simulatedSecondsSinceSample = 0.0;
			lastUpsSample = clock::now();
		}
	}
}

void SimulationEngine::step(double dt, const SimulationConfig& cfg) {
	constexpr std::size_t kDirectForceThreshold = 512;

	const std::size_t workers = desiredWorkers(cfg);
	if (workers != threadPool_.workerCount()) {
		threadPool_.resize(workers);
	}

	int writeIndex = 0;
	{
		std::shared_lock<std::shared_mutex> lock(stateMutex_);
		writeIndex = 1 - readIndex_;
		states_[writeIndex] = states_[readIndex_];
		idMaps_[writeIndex] = idMaps_[readIndex_];
	}

	BodyState& state = states_[writeIndex];
	IdIndexMap& idMap = idMaps_[writeIndex];

	commandQueue_.drainTo(drainedCommands_);
	applyCommands(state, idMap, drainedCommands_);

	const std::size_t n = state.size();
	if (n == 0) {
		std::unique_lock<std::shared_mutex> publishLock(stateMutex_);
		readIndex_ = writeIndex;
		return;
	}

	acc0X_.assign(n, 0.0);
	acc0Y_.assign(n, 0.0);
	predX_.assign(n, 0.0);
	predY_.assign(n, 0.0);
	acc1X_.assign(n, 0.0);
	acc1Y_.assign(n, 0.0);

	const double eps2 = cfg.softeningEpsilon * cfg.softeningEpsilon;
	if (n <= kDirectForceThreshold) {
		auto computeAccelerationDirect =
		    [&](const std::vector<double>& x, const std::vector<double>& y,
		        std::vector<double>& outAx, std::vector<double>& outAy) {
			    for (std::size_t i = 0; i < n; ++i) {
				    double ax = 0.0;
				    double ay = 0.0;
				    for (std::size_t j = 0; j < n; ++j) {
					    if (i == j) {
						    continue;
					    }
					    const double dx = x[j] - x[i];
					    const double dy = y[j] - y[i];
					    const double d2 = dx * dx + dy * dy + eps2;
					    const double d = std::sqrt(d2);
					    const double invD3 = 1.0 / (d2 * d);
					    const double f = cfg.gravitationalConstant * state.mass[j] * invD3;
					    ax += dx * f;
					    ay += dy * f;
				    }
				    outAx[i] = ax;
				    outAy[i] = ay;
			    }
		    };

		computeAccelerationDirect(state.posX, state.posY, acc0X_, acc0Y_);
		for (std::size_t i = 0; i < n; ++i) {
			predX_[i] = state.posX[i] + state.velX[i] * dt + 0.5 * acc0X_[i] * dt * dt;
			predY_[i] = state.posY[i] + state.velY[i] * dt + 0.5 * acc0Y_[i] * dt * dt;
		}
		computeAccelerationDirect(predX_, predY_, acc1X_, acc1Y_);
		for (std::size_t i = 0; i < n; ++i) {
			state.velX[i] += 0.5 * (acc0X_[i] + acc1X_[i]) * dt;
			state.velY[i] += 0.5 * (acc0Y_[i] + acc1Y_[i]) * dt;
			state.posX[i] = predX_[i];
			state.posY[i] = predY_[i];
		}

		overlapPairs_.clear();
		for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(n); ++i) {
			for (std::uint32_t j = i + 1; j < static_cast<std::uint32_t>(n); ++j) {
				const double dx = state.posX[j] - state.posX[i];
				const double dy = state.posY[j] - state.posY[i];
				const double rr = state.radius[i] + state.radius[j];
				if ((dx * dx + dy * dy) <= rr * rr) {
					overlapPairs_.emplace_back(i, j);
				}
			}
		}
		if (!overlapPairs_.empty()) {
			mergeOverlaps(state, idMap, overlapPairs_);
		}

		std::unique_lock<std::shared_mutex> publishLock(stateMutex_);
		readIndex_ = writeIndex;
		std::lock_guard<std::mutex> timerLock(timerMutex_);
		simulatedSeconds_ += dt;
		return;
	}
	currentTree_.build(state.posX, state.posY, state.mass);
	if (currentTree_.empty()) {
		return;
	}

	const std::size_t chunkSize = std::max<std::size_t>(64, n / (workers * 8 + 1));
	threadPool_.parallelFor(n, chunkSize, [&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			double ax = 0.0;
			double ay = 0.0;
			currentTree_.computeAcceleration(i, state.posX[i], state.posY[i], cfg.barnesHutTheta,
			                                 eps2, cfg.gravitationalConstant, ax, ay);
			acc0X_[i] = ax;
			acc0Y_[i] = ay;
			predX_[i] = state.posX[i] + state.velX[i] * dt + 0.5 * ax * dt * dt;
			predY_[i] = state.posY[i] + state.velY[i] * dt + 0.5 * ay * dt * dt;
		}
	});
	predictedTree_.build(predX_, predY_, state.mass);
	threadPool_.parallelFor(n, chunkSize, [&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			double ax = 0.0;
			double ay = 0.0;
			predictedTree_.computeAcceleration(i, predX_[i], predY_[i], cfg.barnesHutTheta, eps2,
			                                   cfg.gravitationalConstant, ax, ay);
			acc1X_[i] = ax;
			acc1Y_[i] = ay;
		}
	});

	threadPool_.parallelFor(n, chunkSize, [&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			state.velX[i] += 0.5 * (acc0X_[i] + acc1X_[i]) * dt;
			state.velY[i] += 0.5 * (acc0Y_[i] + acc1Y_[i]) * dt;
			state.posX[i] = predX_[i];
			state.posY[i] = predY_[i];
		}
	});
	collisionGrid_.build(state.posX, state.posY, state.radius, cfg.collisionCellScale);
	collisionGrid_.findOverlaps(state.posX, state.posY, state.radius, overlapPairs_);
	if (!overlapPairs_.empty()) {
		mergeOverlaps(state, idMap, overlapPairs_);
	}

	std::unique_lock<std::shared_mutex> publishLock(stateMutex_);
	readIndex_ = writeIndex;
	std::lock_guard<std::mutex> timerLock(timerMutex_);
	simulatedSeconds_ += dt;
}

void SimulationEngine::removeBodyAt(BodyState& state, IdIndexMap& idMap, std::uint32_t denseIndex) {
	if (state.empty() || denseIndex >= state.size()) {
		return;
	}
	state.swapRemove(denseIndex);
	idMap.removeDenseIndex(denseIndex);
}

void SimulationEngine::applyCommands(BodyState& state,
                                     IdIndexMap& idMap,
                                     const std::vector<SimCommand>& commands) {
	auto applySpawn = [&](const SpawnCommand& spawn) {
		double mass = clampFinite(spawn.mass, 1.0);
		if (std::abs(mass) < 1e-12) {
			mass = 1.0;
		}
		state.pushBack(spawn.x, spawn.y, spawn.vx, spawn.vy, mass,
		               clampPositive(spawn.radius, 1.0));
		[[maybe_unused]] const BodyId id =
		    idMap.addAtDenseIndex(static_cast<std::uint32_t>(state.size() - 1));
	};

	for (const SimCommand& command : commands) {
		if (command.type == SimCommand::Type::Spawn) {
			applySpawn(command.spawn);
			continue;
		}

		if (command.type == SimCommand::Type::ClearAll) {
			state.clear();
			idMap.clear();
			continue;
		}

		if (command.type == SimCommand::Type::ReplaceWorld) {
			state.clear();
			idMap.clear();
			state.reserve(command.replacementBodies.size());
			for (const SpawnCommand& spawn : command.replacementBodies) {
				applySpawn(spawn);
			}
			continue;
		}

		const std::optional<std::uint32_t> idx = idMap.denseIndexFor(command.del.id);
		if (idx.has_value()) {
			removeBodyAt(state, idMap, *idx);
		}
	}
}

void SimulationEngine::mergeOverlaps(BodyState& state,
                                     IdIndexMap& idMap,
                                     const std::vector<UniformGrid::OverlapPair>& overlaps) {
	if (state.empty() || overlaps.empty()) {
		return;
	}

	UnionFind unionFind(state.size());
	for (const auto& [a, b] : overlaps) {
		if (a < state.size() && b < state.size()) {
			unionFind.unite(a, b);
		}
	}

	std::vector<std::vector<BodyId>> components(state.size());
	for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(state.size()); ++i) {
		const std::size_t root = unionFind.find(i);
		components[root].push_back(idMap.idAtDenseIndex(i));
	}

	for (std::size_t root = 0; root < components.size(); ++root) {
		const std::vector<BodyId>& ids = components[root];
		if (ids.size() < 2) {
			continue;
		}

		std::vector<std::uint32_t> liveIndices;
		liveIndices.reserve(ids.size());
		for (BodyId id : ids) {
			const std::optional<std::uint32_t> idx = idMap.denseIndexFor(id);
			if (idx.has_value()) {
				liveIndices.push_back(*idx);
			}
		}
		if (liveIndices.size() < 2) {
			continue;
		}
		std::sort(liveIndices.begin(), liveIndices.end());

		const std::uint32_t keepIndex = liveIndices.front();
		const BodyId keepId = idMap.idAtDenseIndex(keepIndex);

		double totalMass = 0.0;
		double momentumX = 0.0;
		double momentumY = 0.0;
		double weightedX = 0.0;
		double weightedY = 0.0;
		double volumeTerm = 0.0;

		for (const std::uint32_t idx : liveIndices) {
			const double m = state.mass[idx];
			totalMass += m;
			momentumX += state.velX[idx] * m;
			momentumY += state.velY[idx] * m;
			weightedX += state.posX[idx] * m;
			weightedY += state.posY[idx] * m;
			volumeTerm += state.radius[idx] * state.radius[idx] * state.radius[idx];
		}

		if (std::abs(totalMass) <= 1e-12) {
			continue;
		}

		state.mass[keepIndex] = totalMass;
		state.velX[keepIndex] = momentumX / totalMass;
		state.velY[keepIndex] = momentumY / totalMass;
		state.posX[keepIndex] = weightedX / totalMass;
		state.posY[keepIndex] = weightedY / totalMass;
		state.radius[keepIndex] = std::cbrt(std::max(0.0, volumeTerm));

		for (BodyId id : ids) {
			if (id == keepId) {
				continue;
			}
			const std::optional<std::uint32_t> idx = idMap.denseIndexFor(id);
			if (idx.has_value()) {
				recordMergeRemap(id, keepId);
				removeBodyAt(state, idMap, *idx);
			}
		}
	}
}

void SimulationEngine::recordMergeRemap(BodyId from, BodyId to) {
	if (from == to) {
		return;
	}
	std::lock_guard<std::mutex> lock(mergeMutex_);
	mergeRemapEvents_.emplace_back(from, to);
}

void SimulationEngine::drainMergeRemapEvents(std::vector<std::pair<BodyId, BodyId>>& out) {
	std::lock_guard<std::mutex> lock(mergeMutex_);
	out.clear();
	out.swap(mergeRemapEvents_);
}

SimulationConfig SimulationEngine::currentConfig() const {
	std::lock_guard<std::mutex> lock(configMutex_);
	return config_;
}

void SimulationEngine::updateConfig(const std::function<void(SimulationConfig&)>& fn) {
	std::lock_guard<std::mutex> lock(configMutex_);
	fn(config_);
}

std::size_t SimulationEngine::desiredWorkers(const SimulationConfig& cfg) const {
	if (cfg.workerCount > 0) {
		return cfg.workerCount;
	}

	const unsigned hw = std::thread::hardware_concurrency();
	if (hw <= 2) {
		return 1;
	}
	return static_cast<std::size_t>(hw - 1u);
}

}  // namespace sim
