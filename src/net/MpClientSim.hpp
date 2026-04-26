#pragma once

#include "net/MpConstants.hpp"
#include "sim/BodyId.hpp"
#include "sim/SimulationConfig.hpp"
#include "sim/SimulationEngine.hpp"

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <functional>
#include <mutex>
#include <optional>
#include <stop_token>
#include <thread>
#include <unordered_map>
#include <utility>
#include <vector>

namespace net {

/// Point-in-time engine state for the main thread while `MpClientSim` owns integration (no live
/// `SimulationEngine` reads on the GUI thread).
struct MpClientRenderPublish {
	std::vector<sim::BodySnapshot> bodies;
	sim::SimulationConfig config{};
	double simulationTimeSeconds = 0.0;
	double simulatedSecondsPerUpdate = 0.0;
	double simulatedSecondsPerRealSecond = 0.0;
};

/// Latest ship control / display state mirrored from `MpClient` samples (main thread writes).
struct MpShipReplicaInput {
	float facing = 0.f;
	std::uint8_t thrustForward = 0;
	std::uint8_t thrustPercent = 100;
	float deltaVCurrentMps = 0.f;
	float deltaVMaxMps = 0.f;
	std::uint64_t lastTick = 0;
};

struct WorldSnapshotJob {
	std::uint64_t serverTick = 0;
	std::uint64_t globalPhysicsStep = 0;
	std::vector<sim::AuthoritativeBody> bodies;
};

/// Dedicated multiplayer client simulation thread: fixed wall cadence (~240 Hz), bounded lead
/// vs `serverPhysicsHeadTarget_`, and per-step wall pacing from `(target - head)` (see
/// `kMpClientPace*` in MpConstants). Main thread enqueues work; sim thread calls `advanceFixedStep`
/// only.
class MpClientSim {
   public:
	explicit MpClientSim(sim::SimulationEngine& engine);
	~MpClientSim();

	MpClientSim(const MpClientSim&) = delete;
	MpClientSim& operator=(const MpClientSim&) = delete;

	/// Start background stepping (call after `syncJoin`).
	void start();
	void stop();

	/// Main thread: apply join snapshot + time scale and seed counters (before `start()`).
	void syncJoin(std::uint64_t joinPhysicsStep,
	              double serverTimeScale,
	              double realSecondsPerPhysicsStep,
	              double shipThrustAccel,
	              std::vector<sim::AuthoritativeBody> bodies);

	/// Main thread: enqueue merge deletes (`from` ids), applied on sim before snapshot processing.
	void postMergeDeletes(std::vector<std::pair<sim::BodyId, sim::BodyId>> remaps);

	/// Main thread: enqueue explicit body removals (shell hits, expiry, etc.).
	void postBodyDeleteBatch(std::vector<sim::BodyId> ids);

	/// Main thread: register new server bodies or refresh existing (e.g. late-joining player
	/// ships).
	void postAuthoritativeUpserts(std::vector<sim::AuthoritativeBody> bodies);

	/// Main thread: enqueue dynamics patches (world + ship); may be empty.
	void postDynamicsPatches(std::vector<sim::BodyDynamicsPatch> patches);

	void postAuthorityBundle(std::vector<sim::BodyDynamicsPatch> patches,
	                         std::uint64_t latestAuthorityStep);

	void setLastConfirmedAuthorityStep(std::uint64_t step);

	/// Main thread: full world snapshot at `globalPhysicsStep` (FIFO on sim thread).
	void enqueueWorldSnapshot(WorldSnapshotJob job);

	/// Main thread: full replica map copy (small: ships only).
	void syncReplicas(const std::unordered_map<sim::BodyId, MpShipReplicaInput>& replicas);

	/// Main thread: server `globalPhysicsStep` from the latest network packet (ship and/or world
	/// snapshot), monotonic max. Sim thread treats this as the **target** `clientPhysicsHead`
	/// should track toward for pacing (see `kMpClientPace*` in MpConstants).
	void setServerPhysicsHeadTarget(std::uint64_t serverGlobalPhysicsStep);

	[[nodiscard]] std::uint64_t serverPhysicsHeadTarget() const {
		return serverPhysicsHeadTarget_.load(std::memory_order_acquire);
	}

	/// Global physics step index of the client's integrated world (matches server
	/// `globalPhysicsStep` after each full snapshot fast-forward where `W >= headBefore`).
	[[nodiscard]] std::uint64_t clientPhysicsHead() const {
		return clientPhysicsHead_.load(std::memory_order_acquire);
	}
	[[nodiscard]] std::uint64_t lastConfirmedAuthorityStep() const {
		return lastConfirmedAuthorityStep_.load(std::memory_order_acquire);
	}
	[[nodiscard]] double realSecondsPerPhysicsStep() const { return realPhysicsStep_; }

	/// Main thread: copy last published render state (sim thread only mutates the engine).
	void copyLatestRenderPublish(MpClientRenderPublish& out) const;

	/// Main thread: merge remap events produced on the sim thread (for UI only).
	void takePendingMergeRemaps(std::vector<std::pair<sim::BodyId, sim::BodyId>>& out);

	/// Main thread: pending full-world snapshots not yet consumed by the sim thread.
	[[nodiscard]] std::size_t snapshotJobQueueDepth() const;

	/// Monotonic: sim-thread wakeups that executed the max physics-step budget (catch-up
	/// saturated).
	[[nodiscard]] std::uint64_t simPhysicsWakeCapHitsTotal() const {
		return simPhysicsWakeCapHitsTotal_.load(std::memory_order_relaxed);
	}

#if defined(POTATO_GSIM_MP_WORLD_SYNC_TESTS)
	/// Unit tests: run after `stop()` — drains work then snapshot jobs on the calling thread.
	void testingApplyQueuedNetworkWorkOnCallerThread();
#endif

   private:
	void threadMain(std::stop_token st);
	void drainWorkQueue();
	void drainSnapshotJobQueue();
	void processWorldSnapshotJob(const WorldSnapshotJob& job);
	void publishRenderStateFromEngine();
	void forwardMergeEventsFromEngine();
	void integrateOnePhysicsStep(const std::unordered_map<sim::BodyId, MpShipReplicaInput>& reps,
	                             bool recordReplaySample);

	sim::SimulationEngine& engine_;
	std::optional<std::jthread> thread_;

	std::mutex workMutex_;
	std::deque<std::function<void()>> workQueue_;

	mutable std::mutex snapshotJobMutex_;
	std::deque<WorldSnapshotJob> snapshotJobQueue_;

	std::mutex replicaMutex_;
	std::unordered_map<sim::BodyId, MpShipReplicaInput> replicas_;

	/// (stepIndex, replica thrust map) for rollback replay; capped at 512 entries.
	std::deque<std::pair<std::uint64_t, std::unordered_map<sim::BodyId, MpShipReplicaInput>>>
	    replayInputHistory_;

	mutable std::mutex renderMutex_;
	MpClientRenderPublish renderPublish_;

	std::mutex mergeOutMutex_;
	std::deque<std::vector<std::pair<sim::BodyId, sim::BodyId>>> mergeOutQueue_;

	std::atomic<std::uint64_t> clientPhysicsHead_{0};
	std::atomic<std::uint64_t> lastConfirmedAuthorityStep_{0};
	std::atomic<std::uint64_t> serverPhysicsHeadTarget_{0};
	std::atomic<std::uint64_t> simPhysicsWakeCapHitsTotal_{0};

	double realPhysicsStep_ = kDefaultRealSecondsPerPhysicsStep;
	double shipThrustAccel_ = kDefaultShipThrustAccel;
};

}  // namespace net
