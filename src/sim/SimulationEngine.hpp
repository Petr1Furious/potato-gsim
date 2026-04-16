#pragma once

#include "sim/BodyState.hpp"
#include "sim/CommandQueue.hpp"
#include "sim/IdIndexMap.hpp"
#include "sim/QuadTree.hpp"
#include "sim/SimulationConfig.hpp"
#include "sim/ThreadPool.hpp"
#include "sim/UniformGrid.hpp"

#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <mutex>
#include <optional>
#include <shared_mutex>
#include <stop_token>
#include <thread>
#include <utility>
#include <vector>

namespace sim {

struct BodySnapshot {
	BodyId id = 0;
	double x = 0.0;
	double y = 0.0;
	double vx = 0.0;
	double vy = 0.0;
	double mass = 0.0;
	double radius = 0.0;
};

enum class ForceAlgorithm { DirectSerial, DirectParallel, BarnesHutParallel };

struct EngineDebugStats {
	bool valid = false;
	const char* forceAlgorithm = "direct_serial";
	bool collisionPhaseExecuted = false;
	std::size_t bodyCount = 0;
	std::size_t workerCount = 0;
	std::size_t chunkSize = 0;
	std::size_t directSerialMaxBodies = 0;
	std::size_t directParallelMaxBodies = 0;
	std::size_t collisionStepInterval = 1;
	const char* chunkPolicy = "dynamic";
	std::size_t currentTreeNodes = 0;
	std::size_t predictedTreeNodes = 0;
	std::size_t overlapPairs = 0;
	std::size_t mergedBodies = 0;

	double avgNodeVisitsAcc0 = 0.0;
	double avgNodeVisitsAcc1 = 0.0;
	double avgDirectInteractionsAcc0 = 0.0;
	double avgDirectInteractionsAcc1 = 0.0;
	double avgAggregateApproximationsAcc0 = 0.0;
	double avgAggregateApproximationsAcc1 = 0.0;
	double avgContributionsAcc0 = 0.0;
	double avgContributionsAcc1 = 0.0;

	double commandsMs = 0.0;
	double currentTreeBuildMs = 0.0;
	double acc0Ms = 0.0;
	double predictedTreeBuildMs = 0.0;
	double acc1Ms = 0.0;
	double integrateMs = 0.0;
	double collisionMs = 0.0;
	double mergeMs = 0.0;
	double publishMs = 0.0;
	double totalStepMs = 0.0;
};

class SimulationEngine {
   public:
	explicit SimulationEngine(SimulationConfig config = {});
	~SimulationEngine();

	SimulationEngine(const SimulationEngine&) = delete;
	SimulationEngine& operator=(const SimulationEngine&) = delete;

	void start();
	void stop();

	void queueSpawn(const SpawnCommand& command);
	void queueDelete(BodyId id);
	void queueDeleteNearest(double x, double y, double maxDistance);
	void queueClearAll();
	void queueReplaceWorld(const std::vector<SpawnCommand>& bodies);

	[[nodiscard]] std::optional<BodyId> findNearestBody(double x,
	                                                    double y,
	                                                    double maxDistance) const;
	[[nodiscard]] std::optional<BodySnapshot> bodyById(BodyId id) const;
	[[nodiscard]] std::size_t bodyCount() const;

	void withReadState(const std::function<void(const BodyState&)>& fn) const;
	void withReadSnapshot(const std::function<void(const BodyState&, const IdIndexMap&)>& fn) const;
	void copyBodies(std::vector<BodySnapshot>& out) const;

	[[nodiscard]] SimulationConfig config() const;
	[[nodiscard]] double simulationTimeSeconds() const;
	[[nodiscard]] double updatesPerSecond() const;
	[[nodiscard]] double simulatedSecondsPerRealSecond() const;
	[[nodiscard]] double simulatedSecondsPerUpdate() const;
	[[nodiscard]] EngineDebugStats debugStats() const;
	void resetSimulationTimer();
	void setSimulationTimer(double seconds);

	void setMode(SimulationMode mode);
	void toggleMode();
	void setPaused(bool paused);
	[[nodiscard]] std::uint64_t requestPauseAck(bool paused);
	[[nodiscard]] bool isPauseAcked(std::uint64_t token) const;
	void togglePaused();
	void setTimeScale(double timeScale);
	void scaleTimeBy(double factor);
	void setFixedDt(double fixedDtSeconds);
	void setRealtimeDtOutlierClamp(std::size_t windowSize,
	                               double spikeClampMultiplier,
	                               std::size_t warmupSamples);
	void setGravityConstant(double gravitationalConstant);
	void setCollisionCellScale(double cellScale);
	void setCollisionStepInterval(std::size_t stepInterval);
	void setTheta(double theta);
	void setSoftening(double epsilon);
	void setWorkerCount(std::size_t workers);
	void setParallelChunkSize(std::size_t chunkSize);
	void setChunkPolicy(ChunkPolicy policy);
	void setDirectSerialMaxBodies(std::size_t bodyCount);
	void setDirectParallelMaxBodies(std::size_t bodyCount);
	void setDebugMetricsEnabled(bool enabled);
	[[nodiscard]] bool debugMetricsEnabled() const;

	void seedCircularCloud(std::size_t count, double centerX, double centerY, double spreadRadius);
	void drainMergeRemapEvents(std::vector<std::pair<BodyId, BodyId>>& out);

   private:
	void simulationLoop(std::stop_token stopToken);
	void step(double dt, const SimulationConfig& cfg);
	void publishStepResult(int writeIndex,
	                       double dt,
	                       bool advanceTimer,
	                       const std::chrono::steady_clock::time_point& stepStart,
	                       EngineDebugStats& debug);
	void stepDirectPath(double dt,
	                    const SimulationConfig& cfg,
	                    ForceAlgorithm algorithm,
	                    BodyState& state,
	                    IdIndexMap& idMap,
	                    int writeIndex,
	                    bool runCollisionPhase,
	                    const std::chrono::steady_clock::time_point& stepStart,
	                    EngineDebugStats& debug);
	void stepBarnesHutPath(double dt,
	                       const SimulationConfig& cfg,
	                       BodyState& state,
	                       IdIndexMap& idMap,
	                       int writeIndex,
	                       bool runCollisionPhase,
	                       bool collectTraversalStats,
	                       const std::chrono::steady_clock::time_point& stepStart,
	                       EngineDebugStats& debug);

	static void removeBodyAt(BodyState& state, IdIndexMap& idMap, std::uint32_t denseIndex);
	static void applyCommands(BodyState& state,
	                          IdIndexMap& idMap,
	                          const std::vector<SimCommand>& commands);
	void mergeOverlaps(BodyState& state,
	                   IdIndexMap& idMap,
	                   const std::vector<UniformGrid::OverlapPair>& overlaps);
	void recordMergeRemap(BodyId from, BodyId to);

	[[nodiscard]] SimulationConfig currentConfig() const;
	void updateConfig(const std::function<void(SimulationConfig&)>& fn);
	[[nodiscard]] std::size_t desiredWorkers(const SimulationConfig& cfg) const;
	[[nodiscard]] std::size_t resolveChunkSize(const SimulationConfig& cfg,
	                                           std::size_t bodyCount,
	                                           std::size_t workers) const;
	[[nodiscard]] static ForceAlgorithm selectForceAlgorithm(std::size_t bodyCount,
	                                                         std::size_t workers,
	                                                         const SimulationConfig& cfg);
	[[nodiscard]] static const char* forceAlgorithmLabel(ForceAlgorithm algorithm);
	[[nodiscard]] static const char* chunkPolicyLabel(ChunkPolicy policy);

	mutable std::shared_mutex stateMutex_;
	BodyState states_[2];
	IdIndexMap idMaps_[2];
	int readIndex_ = 0;

	mutable std::mutex configMutex_;
	SimulationConfig config_;

	CommandQueue commandQueue_;
	ThreadPool threadPool_;
	BarnesHutTree currentTree_;
	BarnesHutTree predictedTree_;
	UniformGrid collisionGrid_;

	std::vector<SimCommand> drainedCommands_;
	std::vector<UniformGrid::OverlapPair> overlapPairs_;
	std::vector<double> acc0X_;
	std::vector<double> acc0Y_;
	std::vector<double> predX_;
	std::vector<double> predY_;
	std::vector<double> acc1X_;
	std::vector<double> acc1Y_;
	std::vector<BarnesHutTree::TraversalStats> traversalAcc0_;
	std::vector<BarnesHutTree::TraversalStats> traversalAcc1_;

	std::jthread simulationThread_;
	std::atomic_bool running_{false};
	std::atomic<double> updatesPerSecond_{0.0};
	std::atomic<double> simulatedSecondsPerRealSecond_{0.0};
	std::atomic<double> simulatedSecondsPerUpdate_{0.0};
	std::atomic<std::uint64_t> pauseRequestSeq_{0};
	std::atomic<std::uint64_t> pauseAckSeq_{0};
	std::atomic_bool pauseRequestPausedState_{false};
	std::atomic_bool debugMetricsEnabled_{false};
	std::uint64_t stepCounter_ = 0;
	mutable std::mutex timerMutex_;
	double simulatedSeconds_ = 0.0;
	mutable std::mutex debugMutex_;
	EngineDebugStats debugStats_;
	std::mutex mergeMutex_;
	std::vector<std::pair<BodyId, BodyId>> mergeRemapEvents_;
	mutable std::mutex lifecycleMutex_;
};

}  // namespace sim
