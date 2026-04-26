#include "sim/SimulationEngine.hpp"
#include "sim/UnionFind.hpp"

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <iomanip>
#include <iostream>
#include <mutex>
#include <numbers>
#include <random>
#include <sstream>
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

void findOverlapsDirectSmall(const std::vector<double>& posX,
                             const std::vector<double>& posY,
                             const std::vector<double>& radius,
                             std::vector<UniformGrid::OverlapPair>& outPairs) {
	outPairs.clear();
	const std::size_t n = posX.size();
	if (n < 2) {
		return;
	}
	outPairs.reserve((n * (n - 1)) / 8 + 4);
	for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(n); ++i) {
		for (std::uint32_t j = i + 1; j < static_cast<std::uint32_t>(n); ++j) {
			const double dx = posX[j] - posX[i];
			const double dy = posY[j] - posY[i];
			const double rr = radius[i] + radius[j];
			if ((dx * dx + dy * dy) <= rr * rr) {
				outPairs.emplace_back(i, j);
			}
		}
	}
}

void logStepBreakdown(const EngineDebugStats& debug) {
	std::array<std::pair<const char*, double>, 8> phases{
	    std::pair{"cmd", debug.commandsMs},        std::pair{"tree0", debug.currentTreeBuildMs},
	    std::pair{"acc0", debug.acc0Ms},           std::pair{"tree1", debug.predictedTreeBuildMs},
	    std::pair{"acc1", debug.acc1Ms},           std::pair{"integrate", debug.integrateMs},
	    std::pair{"collision", debug.collisionMs}, std::pair{"merge", debug.mergeMs},
	};
	std::sort(phases.begin(), phases.end(),
	          [](const auto& a, const auto& b) { return a.second > b.second; });

	std::ostringstream line;
	line << std::fixed << std::setprecision(3);
	line << "[sim-step] n=" << debug.bodyCount << " path=" << debug.forceAlgorithm
	     << " workers=" << debug.workerCount << " chunk=" << debug.chunkSize << "("
	     << debug.chunkPolicy << ")"
	     << " coll=" << (debug.collisionPhaseExecuted ? "run" : "skip")
	     << " total=" << debug.totalStepMs << "ms"
	     << " | cmd=" << debug.commandsMs << " tree0=" << debug.currentTreeBuildMs
	     << " acc0=" << debug.acc0Ms << " tree1=" << debug.predictedTreeBuildMs
	     << " acc1=" << debug.acc1Ms << " int=" << debug.integrateMs
	     << " coll=" << debug.collisionMs << " merge=" << debug.mergeMs
	     << " pub=" << debug.publishMs << " | top=" << phases[0].first << ":" << phases[0].second
	     << "ms, " << phases[1].first << ":" << phases[1].second << "ms, " << phases[2].first << ":"
	     << phases[2].second << "ms";

	static std::mutex logMutex;
	std::lock_guard<std::mutex> lock(logMutex);
	std::cout << line.str() << '\n';
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
	stepCounter_ = 0;
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

void SimulationEngine::queueRename(BodyId id, const std::string& name) {
	commandQueue_.pushRename(id, name);
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
		    .name = state.name[i],
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
	    .name = state.name[i],
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

void SimulationEngine::setCollisionStepInterval(std::size_t stepInterval) {
	updateConfig([stepInterval](SimulationConfig& cfg) {
		cfg.collisionStepInterval = std::max<std::size_t>(1, stepInterval);
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

void SimulationEngine::setParallelChunkSize(std::size_t chunkSize) {
	updateConfig([chunkSize](SimulationConfig& cfg) {
		cfg.parallelChunkSize = std::min<std::size_t>(chunkSize, 1u << 20u);
	});
}

void SimulationEngine::setChunkPolicy(ChunkPolicy policy) {
	updateConfig([policy](SimulationConfig& cfg) { cfg.chunkPolicy = policy; });
}

void SimulationEngine::setDirectSerialMaxBodies(std::size_t bodyCount) {
	updateConfig([bodyCount](SimulationConfig& cfg) {
		cfg.directSerialMaxBodies = std::min<std::size_t>(bodyCount, 1u << 22u);
		if (cfg.directParallelMaxBodies < cfg.directSerialMaxBodies) {
			cfg.directParallelMaxBodies = cfg.directSerialMaxBodies;
		}
	});
}

void SimulationEngine::setDirectParallelMaxBodies(std::size_t bodyCount) {
	updateConfig([bodyCount](SimulationConfig& cfg) {
		cfg.directParallelMaxBodies = std::min<std::size_t>(bodyCount, 1u << 22u);
		if (cfg.directSerialMaxBodies > cfg.directParallelMaxBodies) {
			cfg.directSerialMaxBodies = cfg.directParallelMaxBodies;
		}
	});
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

		if (!running_.load(std::memory_order_acquire)) {
			break;
		}

		if (cfg.paused) {
			std::this_thread::sleep_for(std::chrono::milliseconds(1));
			continue;
		}

		double wallDt = std::max(0.0, elapsed);
		if (!std::isfinite(wallDt) || wallDt <= 0.0) {
			continue;
		}
		const double ts =
		    (std::isfinite(cfg.timeScale) && cfg.timeScale > 0.0) ? cfg.timeScale : 1.0;
		// Single-step wall scaling (no rolling window). Cap dt to limit hitches.
		constexpr double kMaxSimStepSeconds = 0.25;
		const double dt = std::min(wallDt * ts, kMaxSimStepSeconds);
		if (!std::isfinite(dt) || dt <= 0.0) {
			continue;
		}
		step(dt, cfg);
		++stepsSinceUpsSample;
		simulatedSecondsSinceSample += dt;

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
			if (dt > 0.0 && std::isfinite(dt)) {
				std::lock_guard<std::mutex> fuelLock(spFuelLedgerMutex_);
				spFuelLedgerAccumSimDt_ += dt;
			}
		}
	}
	const auto publishEnd = clock::now();
	debug.publishMs = durationMs(publishEnd - publishStart);
	debug.totalStepMs = durationMs(publishEnd - stepStart);
	std::lock_guard<std::mutex> debugLock(debugMutex_);
	debugStats_ = debug;
	if (debugMetricsEnabled_.load(std::memory_order_relaxed)) {
		logStepBreakdown(debug);
	}
}

SimulationEngine::CollisionPhaseResult SimulationEngine::runCollisionPhase(
    BodyState& state,
    IdIndexMap& idMap,
    const SimulationConfig& cfg) {
	using clock = std::chrono::steady_clock;
	CollisionPhaseResult out{};
	const auto collisionStart = clock::now();
	const std::size_t n = state.size();
	if (n <= 128) {
		findOverlapsDirectSmall(state.posX, state.posY, state.radius, overlapPairs_);
	} else {
		collisionGrid_.build(state.posX, state.posY, state.radius, cfg.collisionCellScale);
		collisionGrid_.findOverlaps(state.posX, state.posY, state.radius, overlapPairs_);
	}
	const auto collisionEnd = clock::now();
	out.collisionMs = durationMs(collisionEnd - collisionStart);
	out.overlapPairs = overlapPairs_.size();

	const auto mergeStart = clock::now();
	const std::size_t preMergeCount = state.size();
	if (!overlapPairs_.empty()) {
		mergeOverlaps(state, idMap, overlapPairs_);
	}
	const auto mergeEnd = clock::now();
	out.mergeMs = durationMs(mergeEnd - mergeStart);
	out.mergedBodies = preMergeCount > state.size() ? (preMergeCount - state.size()) : 0;
	return out;
}

void SimulationEngine::stepDirectPath(double dt,
                                      const SimulationConfig& cfg,
                                      ForceAlgorithm algorithm,
                                      BodyState& state,
                                      IdIndexMap& idMap,
                                      int writeIndex,
                                      bool runCollisionPhase,
                                      const std::chrono::steady_clock::time_point& stepStart,
                                      EngineDebugStats& debug) {
	using clock = std::chrono::steady_clock;
	const std::size_t n = state.size();
	const std::size_t workers = std::max<std::size_t>(1, debug.workerCount);
	const std::size_t chunkSize = resolveChunkSize(cfg, n, workers);
	const ChunkPolicy chunkPolicy = cfg.chunkPolicy;
	debug.chunkSize = chunkSize;
	debug.chunkPolicy = chunkPolicyLabel(chunkPolicy);
	const bool serialNoPool = algorithm == ForceAlgorithm::DirectSerial || debug.workerCount == 0;
	const auto runRange = [&](auto&& fn) {
		if (serialNoPool) {
			fn(0, n);
			return;
		}
		threadPool_.parallelFor(n, chunkSize, chunkPolicy, fn);
	};

	const double eps2 = cfg.softeningEpsilon * cfg.softeningEpsilon;
	const double halfDt = 0.5 * dt;
	const double halfDt2 = 0.5 * dt * dt;

	const auto acc0Start = clock::now();
	runRange([&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			double ax = 0.0;
			double ay = 0.0;
			const double xi = state.posX[i];
			const double yi = state.posY[i];
			for (std::size_t j = 0; j < n; ++j) {
				if (i == j) {
					continue;
				}
				const double dx = state.posX[j] - xi;
				const double dy = state.posY[j] - yi;
				const double d2 = dx * dx + dy * dy + eps2;
				const double invD = 1.0 / std::sqrt(d2);
				const double invD3 = invD * invD * invD;
				const double f = cfg.gravitationalConstant * state.mass[j] * invD3;
				ax += dx * f;
				ay += dy * f;
			}
			acc0X_[i] = ax;
			acc0Y_[i] = ay;
			predX_[i] = xi + state.velX[i] * dt + ax * halfDt2;
			predY_[i] = yi + state.velY[i] * dt + ay * halfDt2;
		}
	});
	const auto acc0End = clock::now();
	debug.acc0Ms = durationMs(acc0End - acc0Start);

	const auto acc1Start = clock::now();
	runRange([&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			double ax1 = 0.0;
			double ay1 = 0.0;
			const double xi = predX_[i];
			const double yi = predY_[i];
			for (std::size_t j = 0; j < n; ++j) {
				if (i == j) {
					continue;
				}
				const double dx = predX_[j] - xi;
				const double dy = predY_[j] - yi;
				const double d2 = dx * dx + dy * dy + eps2;
				const double invD = 1.0 / std::sqrt(d2);
				const double invD3 = invD * invD * invD;
				const double f = cfg.gravitationalConstant * state.mass[j] * invD3;
				ax1 += dx * f;
				ay1 += dy * f;
			}
			acc1X_[i] = ax1;
			acc1Y_[i] = ay1;
			const double tx = i < thrustX_.size() ? thrustX_[i] : 0.0;
			const double ty = i < thrustY_.size() ? thrustY_[i] : 0.0;
			state.velX[i] += (acc0X_[i] + ax1) * halfDt + tx * dt;
			state.velY[i] += (acc0Y_[i] + ay1) * halfDt + ty * dt;
			state.posX[i] = xi;
			state.posY[i] = yi;
		}
	});
	const auto acc1End = clock::now();
	debug.acc1Ms = durationMs(acc1End - acc1Start);
	debug.integrateMs = 0.0;

	if (runCollisionPhase) {
		const CollisionPhaseResult collision = this->runCollisionPhase(state, idMap, cfg);
		debug.collisionMs += collision.collisionMs;
		debug.mergeMs += collision.mergeMs;
		debug.overlapPairs += collision.overlapPairs;
		debug.mergedBodies += collision.mergedBodies;
	}

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
                                         bool runCollisionPhase,
                                         bool collectTraversalStats,
                                         const std::chrono::steady_clock::time_point& stepStart,
                                         EngineDebugStats& debug) {
	using clock = std::chrono::steady_clock;
	const std::size_t n = state.size();

	if (collectTraversalStats) {
		traversalAcc0_.resize(n);
		traversalAcc1_.resize(n);
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
	const std::size_t chunkSize = resolveChunkSize(cfg, n, workers);
	const ChunkPolicy chunkPolicy = cfg.chunkPolicy;
	debug.chunkSize = chunkSize;
	debug.chunkPolicy = chunkPolicyLabel(chunkPolicy);
	const bool serialNoPool = debug.workerCount == 0;
	const double halfDt = 0.5 * dt;
	const double halfDt2 = 0.5 * dt * dt;
	const auto runRange = [&](auto&& fn) {
		if (serialNoPool) {
			fn(0, n);
			return;
		}
		threadPool_.parallelFor(n, chunkSize, chunkPolicy, fn);
	};
	const auto acc0Start = clock::now();
	runRange([&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			double ax = 0.0;
			double ay = 0.0;
			BarnesHutTree::TraversalStats* stats =
			    collectTraversalStats ? &traversalAcc0_[i] : nullptr;
			currentTree_.computeAcceleration(i, state.posX[i], state.posY[i], cfg.barnesHutTheta,
			                                 eps2, cfg.gravitationalConstant, ax, ay, stats);
			acc0X_[i] = ax;
			acc0Y_[i] = ay;
			predX_[i] = state.posX[i] + state.velX[i] * dt + ax * halfDt2;
			predY_[i] = state.posY[i] + state.velY[i] * dt + ay * halfDt2;
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
	runRange([&](std::size_t begin, std::size_t end) {
		for (std::size_t i = begin; i < end; ++i) {
			double ax = 0.0;
			double ay = 0.0;
			BarnesHutTree::TraversalStats* stats =
			    collectTraversalStats ? &traversalAcc1_[i] : nullptr;
			predictedTree_.computeAcceleration(i, predX_[i], predY_[i], cfg.barnesHutTheta, eps2,
			                                   cfg.gravitationalConstant, ax, ay, stats);
			acc1X_[i] = ax;
			acc1Y_[i] = ay;
			const double tx = i < thrustX_.size() ? thrustX_[i] : 0.0;
			const double ty = i < thrustY_.size() ? thrustY_[i] : 0.0;
			state.velX[i] += (acc0X_[i] + ax) * halfDt + tx * dt;
			state.velY[i] += (acc0Y_[i] + ay) * halfDt + ty * dt;
			state.posX[i] = predX_[i];
			state.posY[i] = predY_[i];
		}
	});
	const auto acc1End = clock::now();
	debug.acc1Ms = durationMs(acc1End - acc1Start);
	debug.integrateMs = 0.0;

	if (collectTraversalStats) {
		double nodeVisits0 = 0.0;
		double nodeVisits1 = 0.0;
		double direct0 = 0.0;
		double direct1 = 0.0;
		double approx0 = 0.0;
		double approx1 = 0.0;
		for (std::size_t i = 0; i < n; ++i) {
			nodeVisits0 += static_cast<double>(traversalAcc0_[i].nodeVisits);
			nodeVisits1 += static_cast<double>(traversalAcc1_[i].nodeVisits);
			direct0 += static_cast<double>(traversalAcc0_[i].directBodyInteractions);
			direct1 += static_cast<double>(traversalAcc1_[i].directBodyInteractions);
			approx0 += static_cast<double>(traversalAcc0_[i].aggregateApproximations);
			approx1 += static_cast<double>(traversalAcc1_[i].aggregateApproximations);
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

	if (runCollisionPhase) {
		const CollisionPhaseResult collision = this->runCollisionPhase(state, idMap, cfg);
		debug.collisionMs += collision.collisionMs;
		debug.mergeMs += collision.mergeMs;
		debug.overlapPairs += collision.overlapPairs;
		debug.mergedBodies += collision.mergedBodies;
	}

	publishStepResult(writeIndex, dt, true, stepStart, debug);
}

void SimulationEngine::advanceFixedStep(const double dt, const SimulationConfig& cfg) {
	step(dt, cfg);
}

void SimulationEngine::applyAuthoritativeSnapshotImmediate(std::vector<AuthoritativeBody> bodies) {
	using clock = std::chrono::steady_clock;
	const auto stepStart = clock::now();
	const SimulationConfig cfg = currentConfig();
	const std::size_t workers = desiredWorkers(cfg);
	if (workers != threadPool_.workerCount()) {
		threadPool_.resize(workers);
	}

	EngineDebugStats debug{};
	debug.valid = true;
	debug.workerCount = workers;
	debug.directSerialMaxBodies = cfg.directSerialMaxBodies;
	debug.directParallelMaxBodies =
	    std::max(cfg.directSerialMaxBodies, cfg.directParallelMaxBodies);
	debug.collisionStepInterval = std::max<std::size_t>(1, cfg.collisionStepInterval);
	debug.collisionPhaseExecuted = (stepCounter_ % debug.collisionStepInterval) == 0;
	debug.chunkPolicy = chunkPolicyLabel(cfg.chunkPolicy);

	int writeIndex = 0;
	{
		std::shared_lock<std::shared_mutex> lock(stateMutex_);
		writeIndex = 1 - readIndex_;
		states_[writeIndex] = states_[readIndex_];
		idMaps_[writeIndex] = idMaps_[readIndex_];
	}

	BodyState& state = states_[writeIndex];
	IdIndexMap& idMap = idMaps_[writeIndex];

	std::vector<SimCommand> cmds;
	cmds.push_back(SimCommand{
	    .type = SimCommand::Type::ApplyAuthoritativeSnapshot,
	    .authoritativeBodies = std::move(bodies),
	});
	const bool spawnedOrReplaced = applyCommands(state, idMap, cmds);
	if (spawnedOrReplaced && state.size() > 1) {
		const CollisionPhaseResult preCollision = runCollisionPhase(state, idMap, cfg);
		debug.collisionMs += preCollision.collisionMs;
		debug.mergeMs += preCollision.mergeMs;
		debug.overlapPairs += preCollision.overlapPairs;
		debug.mergedBodies += preCollision.mergedBodies;
		debug.collisionPhaseExecuted = true;
	}

	debug.bodyCount = state.size();
	publishStepResult(writeIndex, 0.0, false, stepStart, debug);
}

void SimulationEngine::step(double dt, const SimulationConfig& cfg) {
	using clock = std::chrono::steady_clock;
	const auto stepStart = clock::now();

	const std::size_t workers = desiredWorkers(cfg);
	if (workers != threadPool_.workerCount()) {
		threadPool_.resize(workers);
	}

	EngineDebugStats debug{};
	debug.valid = true;
	debug.workerCount = workers;
	debug.directSerialMaxBodies = cfg.directSerialMaxBodies;
	debug.directParallelMaxBodies =
	    std::max(cfg.directSerialMaxBodies, cfg.directParallelMaxBodies);
	debug.collisionStepInterval = std::max<std::size_t>(1, cfg.collisionStepInterval);
	debug.collisionPhaseExecuted = (stepCounter_ % debug.collisionStepInterval) == 0;
	debug.chunkPolicy = chunkPolicyLabel(cfg.chunkPolicy);
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
	const bool spawnedOrReplaced = applyCommands(state, idMap, drainedCommands_);
	const auto commandsEnd = clock::now();
	debug.commandsMs = durationMs(commandsEnd - commandsStart);
	if (spawnedOrReplaced && state.size() > 1) {
		const CollisionPhaseResult preCollision = runCollisionPhase(state, idMap, cfg);
		debug.collisionMs += preCollision.collisionMs;
		debug.mergeMs += preCollision.mergeMs;
		debug.overlapPairs += preCollision.overlapPairs;
		debug.mergedBodies += preCollision.mergedBodies;
		debug.collisionPhaseExecuted = true;
	}

	const std::size_t n = state.size();
	debug.bodyCount = n;
	if (n == 0) {
		debug.forceAlgorithm = forceAlgorithmLabel(ForceAlgorithm::DirectSerial);
		publishStepResult(writeIndex, dt, false, stepStart, debug);
		return;
	}

	acc0X_.resize(n);
	acc0Y_.resize(n);
	predX_.resize(n);
	predY_.resize(n);
	acc1X_.resize(n);
	acc1Y_.resize(n);
	fillThrustArrays(state, idMap);

	const bool runCollisionPhase = debug.collisionPhaseExecuted;
	++stepCounter_;
	const ForceAlgorithm algorithm = selectForceAlgorithm(n, workers, cfg);
	debug.forceAlgorithm = forceAlgorithmLabel(algorithm);
	if (algorithm == ForceAlgorithm::DirectSerial || algorithm == ForceAlgorithm::DirectParallel) {
		stepDirectPath(dt, cfg, algorithm, state, idMap, writeIndex, runCollisionPhase, stepStart,
		               debug);
		return;
	}
	stepBarnesHutPath(dt, cfg, state, idMap, writeIndex, runCollisionPhase, collectTraversalStats,
	                  stepStart, debug);
}

void SimulationEngine::removeBodyAt(BodyState& state, IdIndexMap& idMap, std::uint32_t denseIndex) {
	if (state.empty() || denseIndex >= state.size()) {
		return;
	}
	state.swapRemove(denseIndex);
	idMap.removeDenseIndex(denseIndex);
}

void SimulationEngine::fillThrustArrays(const BodyState& state, const IdIndexMap& idMap) {
	const std::size_t n = state.size();
	thrustX_.assign(n, 0.0);
	thrustY_.assign(n, 0.0);
	std::lock_guard<std::mutex> lock(thrustMutex_);
	for (const auto& [id, a] : pendingThrustAccel_) {
		const std::optional<std::uint32_t> d = idMap.denseIndexFor(id);
		if (d.has_value() && *d < n) {
			thrustX_[*d] += a.first;
			thrustY_[*d] += a.second;
		}
	}
	pendingThrustAccel_.clear();
}

void SimulationEngine::setShipThrustAccelWorld(BodyId id, double ax, double ay) {
	if (id == 0) {
		return;
	}
	std::lock_guard<std::mutex> lock(thrustMutex_);
	pendingThrustAccel_[id] = {ax, ay};
}

void SimulationEngine::queueApplyAuthoritativeSnapshot(std::vector<AuthoritativeBody> bodies) {
	commandQueue_.pushApplyAuthoritativeSnapshot(std::move(bodies));
}

void SimulationEngine::queueUpsertAuthoritativeBodies(std::vector<AuthoritativeBody> bodies) {
	commandQueue_.pushUpsertAuthoritative(std::move(bodies));
}

void SimulationEngine::queuePatchBodyDynamics(std::vector<BodyDynamicsPatch> patches) {
	commandQueue_.pushPatchDynamics(std::move(patches));
}

bool SimulationEngine::applyCommands(BodyState& state,
                                     IdIndexMap& idMap,
                                     const std::vector<SimCommand>& commands) {
	bool spawnedOrReplaced = false;
	auto applySpawn = [&](const SpawnCommand& spawn) {
		double mass = clampFinite(spawn.mass, 1.0);
		if (std::abs(mass) < 1e-12) {
			mass = 1.0;
		}
		state.pushBack(spawn.x, spawn.y, spawn.vx, spawn.vy, mass, clampPositive(spawn.radius, 1.0),
		               spawn.name);
		[[maybe_unused]] const BodyId id =
		    idMap.addAtDenseIndex(static_cast<std::uint32_t>(state.size() - 1));
	};

	for (const SimCommand& command : commands) {
		if (command.type == SimCommand::Type::Spawn) {
			spawnedOrReplaced = true;
			applySpawn(command.spawn);
			continue;
		}

		if (command.type == SimCommand::Type::ApplyAuthoritativeSnapshot) {
			spawnedOrReplaced = true;
			state.clear();
			idMap.clear();
			state.reserve(command.authoritativeBodies.size());
			for (const AuthoritativeBody& b : command.authoritativeBodies) {
				state.pushBack(b.x, b.y, b.vx, b.vy, b.mass, b.radius, b.name);
				idMap.pushServerBody(b.id);
			}
			continue;
		}

		if (command.type == SimCommand::Type::ClearAll) {
			state.clear();
			idMap.clear();
			continue;
		}

		if (command.type == SimCommand::Type::ReplaceWorld) {
			spawnedOrReplaced = spawnedOrReplaced || !command.replacementBodies.empty();
			state.clear();
			idMap.clear();
			state.reserve(command.replacementBodies.size());
			for (const SpawnCommand& spawn : command.replacementBodies) {
				applySpawn(spawn);
			}
			continue;
		}

		if (command.type == SimCommand::Type::Rename) {
			const std::optional<std::uint32_t> idx = idMap.denseIndexFor(command.rename.id);
			if (idx.has_value()) {
				state.name[*idx] = command.rename.name;
			}
			continue;
		}

		if (command.type == SimCommand::Type::PatchDynamics) {
			for (const BodyDynamicsPatch& p : command.dynamicPatches) {
				const std::optional<std::uint32_t> idx = idMap.denseIndexFor(p.id);
				if (!idx.has_value() || *idx >= state.size()) {
					continue;
				}
				const std::uint32_t i = *idx;
				state.posX[i] = p.x;
				state.posY[i] = p.y;
				state.velX[i] = p.vx;
				state.velY[i] = p.vy;
			}
			continue;
		}

		if (command.type == SimCommand::Type::UpsertAuthoritative) {
			for (const AuthoritativeBody& b : command.upsertBodies) {
				double mass = clampFinite(b.mass, 1.0);
				if (std::abs(mass) < 1e-12) {
					mass = 1.0;
				}
				const double radius = clampPositive(b.radius, 1.0);
				const std::optional<std::uint32_t> idx = idMap.denseIndexFor(b.id);
				if (idx.has_value() && *idx < state.size()) {
					const std::uint32_t i = *idx;
					state.posX[i] = b.x;
					state.posY[i] = b.y;
					state.velX[i] = b.vx;
					state.velY[i] = b.vy;
					state.mass[i] = mass;
					state.radius[i] = radius;
					state.name[i] = b.name;
				} else {
					spawnedOrReplaced = true;
					state.pushBack(b.x, b.y, b.vx, b.vy, mass, radius, b.name);
					idMap.pushServerBody(b.id);
				}
			}
			continue;
		}

		if (command.type == SimCommand::Type::Delete) {
			const std::optional<std::uint32_t> idx = idMap.denseIndexFor(command.del.id);
			if (idx.has_value()) {
				removeBodyAt(state, idMap, *idx);
			}
			continue;
		}
	}
	return spawnedOrReplaced;
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
		std::uint32_t keepIndex = liveIndices.front();
		BodyId keepId = idMap.idAtDenseIndex(keepIndex);
		double keepMassAbs = std::abs(state.mass[keepIndex]);
		for (const std::uint32_t idx : liveIndices) {
			const double massAbs = std::abs(state.mass[idx]);
			const BodyId id = idMap.idAtDenseIndex(idx);
			const bool betterMass = massAbs > keepMassAbs;
			const bool massTie = std::abs(massAbs - keepMassAbs) <= 1e-12;
			const bool betterTieBreak = massTie && id < keepId;
			if (betterMass || betterTieBreak) {
				keepIndex = idx;
				keepId = id;
				keepMassAbs = massAbs;
			}
		}

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

double SimulationEngine::takeAccumulatedSimulatedSecondsForShipFuel() {
	std::lock_guard<std::mutex> lock(spFuelLedgerMutex_);
	const double t = spFuelLedgerAccumSimDt_;
	spFuelLedgerAccumSimDt_ = 0.0;
	return t;
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

std::size_t SimulationEngine::resolveChunkSize(const SimulationConfig& cfg,
                                               std::size_t bodyCount,
                                               std::size_t workers) const {
	if (cfg.parallelChunkSize > 0) {
		return std::max<std::size_t>(1, cfg.parallelChunkSize);
	}
	const std::size_t safeWorkers = std::max<std::size_t>(1, workers);
	const std::size_t autoChunk = bodyCount / (safeWorkers * 8 + 1);
	return std::max<std::size_t>(32, autoChunk);
}

ForceAlgorithm SimulationEngine::selectForceAlgorithm(std::size_t bodyCount,
                                                      std::size_t workers,
                                                      const SimulationConfig& cfg) {
	const std::size_t serialMax = cfg.directSerialMaxBodies;
	const std::size_t parallelMax = std::max(serialMax, cfg.directParallelMaxBodies);
	if (bodyCount <= serialMax) {
		return ForceAlgorithm::DirectSerial;
	}
	if (bodyCount <= parallelMax && workers > 0) {
		return ForceAlgorithm::DirectParallel;
	}
	if (bodyCount <= parallelMax) {
		return ForceAlgorithm::DirectSerial;
	}
	return ForceAlgorithm::BarnesHutParallel;
}

const char* SimulationEngine::forceAlgorithmLabel(ForceAlgorithm algorithm) {
	switch (algorithm) {
		case ForceAlgorithm::DirectSerial:
			return "direct_serial";
		case ForceAlgorithm::DirectParallel:
			return "direct_parallel";
		case ForceAlgorithm::BarnesHutParallel:
		default:
			return "barnes_hut_parallel";
	}
}

const char* SimulationEngine::chunkPolicyLabel(ChunkPolicy policy) {
	switch (policy) {
		case ChunkPolicy::StaticCyclic:
			return "static";
		case ChunkPolicy::DynamicClaim:
		default:
			return "dynamic";
	}
}

}  // namespace sim
