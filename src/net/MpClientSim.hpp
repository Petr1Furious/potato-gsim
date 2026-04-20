#pragma once

#include "sim/BodyId.hpp"
#include "sim/SimulationConfig.hpp"
#include "sim/SimulationEngine.hpp"

#include <atomic>
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
	std::uint8_t thrustReverse = 0;
	std::uint64_t lastTick = 0;
};

/// Dedicated multiplayer client simulation thread: fixed wall cadence (~240 Hz), bounded lead,
/// soft pacing near the lead cap. Main thread enqueues network-driven work; sim thread calls
/// `advanceFixedStep` only.
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
	              std::vector<sim::AuthoritativeBody> bodies);

	/// Main thread: enqueue merge deletes (`from` ids), applied in order before dynamics patches
	/// posted in the same frame.
	void postMergeDeletes(std::vector<std::pair<sim::BodyId, sim::BodyId>> remaps);

	/// Main thread: enqueue dynamics patches (world + ship); may be empty.
	void postDynamicsPatches(std::vector<sim::BodyDynamicsPatch> patches);

	/// Apply all dynamics patches for one logical network frame, then advance the authority
	/// watermark — **single queue item** so the sim thread cannot `advanceFixedStep` between
	/// world-only and ship-only applies (which desyncs co-moving bodies).
	void postAuthorityBundle(std::vector<sim::BodyDynamicsPatch> patches,
	                         std::uint64_t latestAuthorityStep);

	/// Main thread: monotonic authority step when there are **no** dynamics patches this frame.
	void setLastConfirmedAuthorityStep(std::uint64_t step);

	/// Main thread: full replica map copy (small: ships only).
	void syncReplicas(const std::unordered_map<sim::BodyId, MpShipReplicaInput>& replicas);

	[[nodiscard]] std::uint64_t clientPhysicsHead() const {
		return clientPhysicsHead_.load(std::memory_order_acquire);
	}
	[[nodiscard]] std::uint64_t lastConfirmedAuthorityStep() const {
		return lastConfirmedAuthorityStep_.load(std::memory_order_acquire);
	}

	/// Main thread: copy last published render state (sim thread only mutates the engine).
	void copyLatestRenderPublish(MpClientRenderPublish& out) const;

	/// Main thread: merge remap events produced on the sim thread (for UI only).
	void takePendingMergeRemaps(std::vector<std::pair<sim::BodyId, sim::BodyId>>& out);

   private:
	void threadMain(std::stop_token st);
	/// Apply all pending main-thread work in FIFO order (see implementation comment for
	/// ordering vs `advanceFixedStep`).
	void drainWorkQueue();
	void publishRenderStateFromEngine();
	void forwardMergeEventsFromEngine();

	sim::SimulationEngine& engine_;
	std::optional<std::jthread> thread_;

	std::mutex workMutex_;
	std::deque<std::function<void()>> workQueue_;

	std::mutex replicaMutex_;
	std::unordered_map<sim::BodyId, MpShipReplicaInput> replicas_;

	mutable std::mutex renderMutex_;
	MpClientRenderPublish renderPublish_;

	std::mutex mergeOutMutex_;
	std::deque<std::vector<std::pair<sim::BodyId, sim::BodyId>>> mergeOutQueue_;

	std::atomic<std::uint64_t> clientPhysicsHead_{0};
	std::atomic<std::uint64_t> lastConfirmedAuthorityStep_{0};
};

}  // namespace net
