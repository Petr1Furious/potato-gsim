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

[[nodiscard]] double durationMs(std::chrono::steady_clock::duration duration) {
	return std::chrono::duration<double, std::milli>(duration).count();
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

EngineDebugStats SimulationEngine::debugStats() const {
	std::lock_guard<std::mutex> lock(debugMutex_);
	return debugStats_;
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
	updateConfig([this, mode](SimulationConfig& cfg) {
		if (cfg.mode == mode) {
			return;
		}
		double ups = updatesPerSecond_.load(std::memory_order_relaxed);
		if (!std::isfinite(ups) || ups < 1.0) {
			ups = 1.0;
		}
		if (cfg.mode == SimulationMode::RealTimeVariableStep &&
		    mode == SimulationMode::DeterministicFixedStep) {
			cfg.timeScale /= ups;
		} else if (cfg.mode == SimulationMode::DeterministicFixedStep &&
		           mode == SimulationMode::RealTimeVariableStep) {
			cfg.timeScale *= ups;
		}
		if (!std::isfinite(cfg.timeScale) || cfg.timeScale <= 0.0) {
			cfg.timeScale = 1.0;
		}
		cfg.mode = mode;
	});
}

void SimulationEngine::toggleMode() {
	const SimulationConfig cfg = currentConfig();
	setMode(cfg.mode == SimulationMode::DeterministicFixedStep
	            ? SimulationMode::RealTimeVariableStep
	            : SimulationMode::DeterministicFixedStep);
}

void SimulationEngine::setPaused(bool paused) {
	updateConfig([paused](SimulationConfig& cfg) { cfg.paused = paused; });
}

std::uint64_t SimulationEngine::requestPauseAck(bool paused) {
	updateConfig([paused](SimulationConfig& cfg) { cfg.paused = paused; });
	pauseRequestPausedState_.store(paused, std::memory_order_release);
	return pauseRequestSeq_.fetch_add(1, std::memory_order_acq_rel) + 1;
}

bool SimulationEngine::isPauseAcked(std::uint64_t token) const {
	return pauseAckSeq_.load(std::memory_order_acquire) >= token;
}

void SimulationEngine::togglePaused() {
	updateConfig([](SimulationConfig& cfg) { cfg.paused = !cfg.paused; });
}

void SimulationEngine::setTimeScale(double timeScale) {
	updateConfig([timeScale](SimulationConfig& cfg) {
		if (std::isfinite(timeScale) && timeScale > 0.0) {
			cfg.timeScale = timeScale;
		}
	});
}

void SimulationEngine::scaleTimeBy(double factor) {
	updateConfig([factor](SimulationConfig& cfg) {
		if (!std::isfinite(factor) || factor <= 0.0) {
			return;
		}
		const double next = cfg.timeScale * factor;
		if (std::isfinite(next) && next > 0.0) {
			cfg.timeScale = next;
		}
	});
}

void SimulationEngine::setFixedDt(double fixedDtSeconds) {
	updateConfig([fixedDtSeconds](SimulationConfig& cfg) {
		if (std::isfinite(fixedDtSeconds) && fixedDtSeconds > 0.0) {
			cfg.fixedDtSeconds = fixedDtSeconds;
		}
	});
}

void SimulationEngine::setRealtimeDtOutlierClamp(std::size_t windowSize,
                                                 double spikeClampMultiplier,
                                                 std::size_t warmupSamples) {
	updateConfig([windowSize, spikeClampMultiplier, warmupSamples](SimulationConfig& cfg) {
		const std::size_t safeWindow = std::max<std::size_t>(1, windowSize);
		const std::size_t safeWarmup =
		    std::min(safeWindow, std::max<std::size_t>(1, warmupSamples));
		if (std::isfinite(spikeClampMultiplier) && spikeClampMultiplier >= 1.0) {
			cfg.realtimeDtSpikeClampMultiplier = spikeClampMultiplier;
		}
		cfg.realtimeDtWindowSize = safeWindow;
		cfg.realtimeDtClampWarmupSamples = safeWarmup;
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

void SimulationEngine::setDebugMetricsEnabled(bool enabled) {
	debugMetricsEnabled_.store(enabled, std::memory_order_relaxed);
}

bool SimulationEngine::debugMetricsEnabled() const {
	return debugMetricsEnabled_.load(std::memory_order_relaxed);
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
	std::uint64_t lastPauseAckedSeq = pauseAckSeq_.load(std::memory_order_relaxed);
	std::size_t stepsSinceUpsSample = 0;
	double simulatedSecondsSinceSample = 0.0;
	SimulationMode lastMode = currentConfig().mode;
	std::vector<double> realtimeDtWindow;
	std::size_t realtimeDtWrite = 0;
	std::size_t realtimeDtCount = 0;
	double realtimeDtSum = 0.0;

	while (!stopToken.stop_requested()) {
		const SimulationConfig cfg = currentConfig();
		const std::uint64_t pauseRequestSeq = pauseRequestSeq_.load(std::memory_order_acquire);
		if (pauseRequestSeq != lastPauseAckedSeq) {
			const bool requestedPaused = pauseRequestPausedState_.load(std::memory_order_acquire);
			if (cfg.paused == requestedPaused) {
				lastPauseAckedSeq = pauseRequestSeq;
				pauseAckSeq_.store(lastPauseAckedSeq, std::memory_order_release);
			}
		}
		const auto now = clock::now();
		const double elapsed = std::chrono::duration<double>(now - lastTick).count();
		lastTick = now;

		if (cfg.mode != lastMode) {
			lastMode = cfg.mode;
			stepsSinceUpsSample = 0;
			simulatedSecondsSinceSample = 0.0;
			lastUpsSample = now;
			realtimeDtWrite = 0;
			realtimeDtCount = 0;
			realtimeDtSum = 0.0;
			realtimeDtWindow.clear();
			updatesPerSecond_.store(0.0, std::memory_order_relaxed);
			simulatedSecondsPerRealSecond_.store(0.0, std::memory_order_relaxed);
			simulatedSecondsPerUpdate_.store(0.0, std::memory_order_relaxed);
		}

		if (!running_.load(std::memory_order_acquire)) {
			break;
		}

		if (cfg.paused) {
			std::this_thread::sleep_for(std::chrono::milliseconds(1));
			continue;
		}

		if (cfg.mode == SimulationMode::DeterministicFixedStep) {
			// Legacy-accurate mode: fixed simulation seconds per update.
			const double dt =
			    std::isfinite(cfg.timeScale) && cfg.timeScale > 0.0 ? cfg.timeScale : 1.0;
			step(dt, cfg);
			++stepsSinceUpsSample;
			simulatedSecondsSinceSample += dt;
		} else {
			const std::size_t windowSize = std::max<std::size_t>(1, cfg.realtimeDtWindowSize);
			if (realtimeDtWindow.size() != windowSize) {
				realtimeDtWindow.assign(windowSize, 0.0);
				realtimeDtWrite = 0;
				realtimeDtCount = 0;
				realtimeDtSum = 0.0;
			}

			double wallDt = std::max(0.0, elapsed);
			if (!std::isfinite(wallDt) || wallDt <= 0.0) {
				continue;
			}

			double avgDt = wallDt;
			if (realtimeDtCount > 0) {
				avgDt = realtimeDtSum / static_cast<double>(realtimeDtCount);
			}
			const std::size_t warmup =
			    std::min(windowSize, std::max<std::size_t>(1, cfg.realtimeDtClampWarmupSamples));
			const double spikeMultiplier = (std::isfinite(cfg.realtimeDtSpikeClampMultiplier) &&
			                                cfg.realtimeDtSpikeClampMultiplier >= 1.0)
			                                   ? cfg.realtimeDtSpikeClampMultiplier
			                                   : 1.0;
			const bool hasFullRealtimeDtWindow = realtimeDtCount >= windowSize;
			if (hasFullRealtimeDtWindow && realtimeDtCount >= warmup) {
				wallDt = std::min(wallDt, avgDt * spikeMultiplier);
			}

			if (realtimeDtCount < windowSize) {
				realtimeDtWindow[realtimeDtWrite] = wallDt;
				realtimeDtSum += wallDt;
				++realtimeDtCount;
			} else {
				realtimeDtSum -= realtimeDtWindow[realtimeDtWrite];
				realtimeDtWindow[realtimeDtWrite] = wallDt;
				realtimeDtSum += wallDt;
			}
			realtimeDtWrite = (realtimeDtWrite + 1) % windowSize;

			const double dt =
			    wallDt *
			    (std::isfinite(cfg.timeScale) && cfg.timeScale > 0.0 ? cfg.timeScale : 1.0);
			if (!std::isfinite(dt) || dt <= 0.0) {
				continue;
			}
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

void SimulationEngine::publishStepResult(int writeIndex,
                                         double dt,
                                         bool advanceTimer,
                                         const std::chrono::steady_clock::time_point& stepStart,
                                         EngineDebugStats& debug) {
	using clock = std::chrono::steady_clock;
	const auto publishStart = clock::now();
	{
		std::unique_lock<std::shared_mutex> publishLock(stateMutex_);
		readIndex_ = writeIndex;
		if (advanceTimer) {
			std::lock_guard<std::mutex> timerLock(timerMutex_);
			simulatedSeconds_ += dt;
		}
	}
	const auto publishEnd = clock::now();
	debug.publishMs = durationMs(publishEnd - publishStart);
	debug.totalStepMs = durationMs(publishEnd - stepStart);
	std::lock_guard<std::mutex> debugLock(debugMutex_);
	debugStats_ = debug;
}

void SimulationEngine::stepDirectPath(double dt,
                                      const SimulationConfig& cfg,
                                      BodyState& state,
                                      IdIndexMap& idMap,
                                      int writeIndex,
                                      const std::chrono::steady_clock::time_point& stepStart,
                                      EngineDebugStats& debug) {
	using clock = std::chrono::steady_clock;
	const std::size_t n = state.size();
	debug.usedDirectPath = true;

	const double eps2 = cfg.softeningEpsilon * cfg.softeningEpsilon;
	auto computeAccelerationDirect = [&](const std::vector<double>& x, const std::vector<double>& y,
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
				const double invD = 1.0 / std::sqrt(d2);
				const double invD3 = invD * invD * invD;
				const double f = cfg.gravitationalConstant * state.mass[j] * invD3;
				ax += dx * f;
				ay += dy * f;
			}
			outAx[i] = ax;
			outAy[i] = ay;
		}
	};

	const auto acc0Start = clock::now();
	computeAccelerationDirect(state.posX, state.posY, acc0X_, acc0Y_);
	const auto acc0End = clock::now();
	debug.acc0Ms = durationMs(acc0End - acc0Start);

	for (std::size_t i = 0; i < n; ++i) {
		predX_[i] = state.posX[i] + state.velX[i] * dt + 0.5 * acc0X_[i] * dt * dt;
		predY_[i] = state.posY[i] + state.velY[i] * dt + 0.5 * acc0Y_[i] * dt * dt;
	}

	const auto acc1Start = clock::now();
	computeAccelerationDirect(predX_, predY_, acc1X_, acc1Y_);
	const auto acc1End = clock::now();
	debug.acc1Ms = durationMs(acc1End - acc1Start);

	const auto integrateStart = clock::now();
	for (std::size_t i = 0; i < n; ++i) {
		state.velX[i] += 0.5 * (acc0X_[i] + acc1X_[i]) * dt;
		state.velY[i] += 0.5 * (acc0Y_[i] + acc1Y_[i]) * dt;
		state.posX[i] = predX_[i];
		state.posY[i] = predY_[i];
	}
	const auto integrateEnd = clock::now();
	debug.integrateMs = durationMs(integrateEnd - integrateStart);

	const auto collisionStart = clock::now();
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
	const auto collisionEnd = clock::now();
	debug.collisionMs = durationMs(collisionEnd - collisionStart);
	debug.overlapPairs = overlapPairs_.size();

	const auto mergeStart = clock::now();
	const std::size_t preMergeCount = state.size();
	if (!overlapPairs_.empty()) {
		mergeOverlaps(state, idMap, overlapPairs_);
	}
	const auto mergeEnd = clock::now();
	debug.mergeMs = durationMs(mergeEnd - mergeStart);
	debug.mergedBodies = preMergeCount > state.size() ? (preMergeCount - state.size()) : 0;

	const double directContribs = static_cast<double>(n > 0 ? (n - 1) : 0);
	debug.avgDirectInteractionsAcc0 = directContribs;
	debug.avgDirectInteractionsAcc1 = directContribs;
	debug.avgContributionsAcc0 = directContribs;
	debug.avgContributionsAcc1 = directContribs;

	publishStepResult(writeIndex, dt, true, stepStart, debug);
}

void SimulationEngine::stepBarnesHutPath(double dt,
                                         const SimulationConfig& cfg,
                                         BodyState& state,
                                         IdIndexMap& idMap,
                                         int writeIndex,
                                         bool collectTraversalStats,
                                         const std::chrono::steady_clock::time_point& stepStart,
                                         EngineDebugStats& debug) {
	using clock = std::chrono::steady_clock;
	const std::size_t n = state.size();
	debug.usedDirectPath = false;

	std::vector<BarnesHutTree::TraversalStats> traversalAcc0;
	std::vector<BarnesHutTree::TraversalStats> traversalAcc1;
	if (collectTraversalStats) {
		traversalAcc0.resize(n);
		traversalAcc1.resize(n);
	}

	const double eps2 = cfg.softeningEpsilon * cfg.softeningEpsilon;
	const auto currentTreeBuildStart = clock::now();
	currentTree_.build(state.posX, state.posY, state.mass);
	const auto currentTreeBuildEnd = clock::now();
	debug.currentTreeBuildMs = durationMs(currentTreeBuildEnd - currentTreeBuildStart);
	debug.currentTreeNodes = currentTree_.nodeCount();
	if (currentTree_.empty()) {
		publishStepResult(writeIndex, dt, false, stepStart, debug);
		return;
	}

	const std::size_t workers = std::max<std::size_t>(1, debug.workerCount);
	const std::size_t chunkSize = std::max<std::size_t>(64, n / (workers * 8 + 1));
	const auto acc0Start = clock::now();
	threadPool_.parallelFor(n, chunkSize, [&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			double ax = 0.0;
			double ay = 0.0;
			BarnesHutTree::TraversalStats* stats =
			    collectTraversalStats ? &traversalAcc0[i] : nullptr;
			currentTree_.computeAcceleration(i, state.posX[i], state.posY[i], cfg.barnesHutTheta,
			                                 eps2, cfg.gravitationalConstant, ax, ay, stats);
			acc0X_[i] = ax;
			acc0Y_[i] = ay;
			predX_[i] = state.posX[i] + state.velX[i] * dt + 0.5 * ax * dt * dt;
			predY_[i] = state.posY[i] + state.velY[i] * dt + 0.5 * ay * dt * dt;
		}
	});
	const auto acc0End = clock::now();
	debug.acc0Ms = durationMs(acc0End - acc0Start);

	const auto predictedTreeBuildStart = clock::now();
	predictedTree_.build(predX_, predY_, state.mass);
	const auto predictedTreeBuildEnd = clock::now();
	debug.predictedTreeBuildMs = durationMs(predictedTreeBuildEnd - predictedTreeBuildStart);
	debug.predictedTreeNodes = predictedTree_.nodeCount();

	const auto acc1Start = clock::now();
	threadPool_.parallelFor(n, chunkSize, [&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			double ax = 0.0;
			double ay = 0.0;
			BarnesHutTree::TraversalStats* stats =
			    collectTraversalStats ? &traversalAcc1[i] : nullptr;
			predictedTree_.computeAcceleration(i, predX_[i], predY_[i], cfg.barnesHutTheta, eps2,
			                                   cfg.gravitationalConstant, ax, ay, stats);
			acc1X_[i] = ax;
			acc1Y_[i] = ay;
		}
	});
	const auto acc1End = clock::now();
	debug.acc1Ms = durationMs(acc1End - acc1Start);

	if (collectTraversalStats) {
		double nodeVisits0 = 0.0;
		double nodeVisits1 = 0.0;
		double direct0 = 0.0;
		double direct1 = 0.0;
		double approx0 = 0.0;
		double approx1 = 0.0;
		for (std::size_t i = 0; i < n; ++i) {
			nodeVisits0 += static_cast<double>(traversalAcc0[i].nodeVisits);
			nodeVisits1 += static_cast<double>(traversalAcc1[i].nodeVisits);
			direct0 += static_cast<double>(traversalAcc0[i].directBodyInteractions);
			direct1 += static_cast<double>(traversalAcc1[i].directBodyInteractions);
			approx0 += static_cast<double>(traversalAcc0[i].aggregateApproximations);
			approx1 += static_cast<double>(traversalAcc1[i].aggregateApproximations);
		}
		const double invN = 1.0 / static_cast<double>(n);
		debug.avgNodeVisitsAcc0 = nodeVisits0 * invN;
		debug.avgNodeVisitsAcc1 = nodeVisits1 * invN;
		debug.avgDirectInteractionsAcc0 = direct0 * invN;
		debug.avgDirectInteractionsAcc1 = direct1 * invN;
		debug.avgAggregateApproximationsAcc0 = approx0 * invN;
		debug.avgAggregateApproximationsAcc1 = approx1 * invN;
		debug.avgContributionsAcc0 = (direct0 + approx0) * invN;
		debug.avgContributionsAcc1 = (direct1 + approx1) * invN;
	}

	const auto integrateStart = clock::now();
	threadPool_.parallelFor(n, chunkSize, [&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			state.velX[i] += 0.5 * (acc0X_[i] + acc1X_[i]) * dt;
			state.velY[i] += 0.5 * (acc0Y_[i] + acc1Y_[i]) * dt;
			state.posX[i] = predX_[i];
			state.posY[i] = predY_[i];
		}
	});
	const auto integrateEnd = clock::now();
	debug.integrateMs = durationMs(integrateEnd - integrateStart);

	const auto collisionStart = clock::now();
	collisionGrid_.build(state.posX, state.posY, state.radius, cfg.collisionCellScale);
	collisionGrid_.findOverlaps(state.posX, state.posY, state.radius, overlapPairs_);
	const auto collisionEnd = clock::now();
	debug.collisionMs = durationMs(collisionEnd - collisionStart);
	debug.overlapPairs = overlapPairs_.size();

	const auto mergeStart = clock::now();
	const std::size_t preMergeCount = state.size();
	if (!overlapPairs_.empty()) {
		mergeOverlaps(state, idMap, overlapPairs_);
	}
	const auto mergeEnd = clock::now();
	debug.mergeMs = durationMs(mergeEnd - mergeStart);
	debug.mergedBodies = preMergeCount > state.size() ? (preMergeCount - state.size()) : 0;

	publishStepResult(writeIndex, dt, true, stepStart, debug);
}

void SimulationEngine::step(double dt, const SimulationConfig& cfg) {
	constexpr std::size_t kDirectForceThreshold = 512;
	using clock = std::chrono::steady_clock;
	const auto stepStart = clock::now();

	const std::size_t workers = desiredWorkers(cfg);
	if (workers != threadPool_.workerCount()) {
		threadPool_.resize(workers);
	}

	EngineDebugStats debug{};
	debug.valid = true;
	debug.workerCount = workers;
	const bool collectTraversalStats = debugMetricsEnabled_.load(std::memory_order_relaxed);

	int writeIndex = 0;
	{
		std::shared_lock<std::shared_mutex> lock(stateMutex_);
		writeIndex = 1 - readIndex_;
		states_[writeIndex] = states_[readIndex_];
		idMaps_[writeIndex] = idMaps_[readIndex_];
	}

	BodyState& state = states_[writeIndex];
	IdIndexMap& idMap = idMaps_[writeIndex];

	const auto commandsStart = clock::now();
	commandQueue_.drainTo(drainedCommands_);
	applyCommands(state, idMap, drainedCommands_);
	const auto commandsEnd = clock::now();
	debug.commandsMs = durationMs(commandsEnd - commandsStart);

	const std::size_t n = state.size();
	debug.bodyCount = n;
	if (n == 0) {
		publishStepResult(writeIndex, dt, false, stepStart, debug);
		return;
	}

	acc0X_.assign(n, 0.0);
	acc0Y_.assign(n, 0.0);
	predX_.assign(n, 0.0);
	predY_.assign(n, 0.0);
	acc1X_.assign(n, 0.0);
	acc1Y_.assign(n, 0.0);

	if (n <= kDirectForceThreshold) {
		stepDirectPath(dt, cfg, state, idMap, writeIndex, stepStart, debug);
		return;
	}
	stepBarnesHutPath(dt, cfg, state, idMap, writeIndex, collectTraversalStats, stepStart, debug);
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
