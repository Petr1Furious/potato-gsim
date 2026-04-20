#include "net/MpClientSim.hpp"
#include "net/MpConstants.hpp"

#include <algorithm>
#include <cmath>

namespace net {

namespace {

void applyReplicaThrustImpl(sim::SimulationEngine& engine,
                            const std::unordered_map<sim::BodyId, MpShipReplicaInput>& replicas) {
	for (const auto& [id, rep] : replicas) {
		const double f = static_cast<double>(rep.facing);
		const double ca = std::cos(f);
		const double sa = std::sin(f);
		double ax = 0.0;
		double ay = 0.0;
		if (rep.thrustForward) {
			ax += net::kShipThrustAccel * ca;
			ay += net::kShipThrustAccel * sa;
		}
		if (rep.thrustReverse) {
			ax -= net::kShipThrustAccel * ca * 0.5;
			ay -= net::kShipThrustAccel * sa * 0.5;
		}
		engine.setShipThrustAccelWorld(id, ax, ay);
	}
}

}  // namespace

MpClientSim::MpClientSim(sim::SimulationEngine& engine) : engine_(engine) {}

MpClientSim::~MpClientSim() {
	stop();
}

void MpClientSim::start() {
	if (thread_.has_value()) {
		return;
	}
	thread_.emplace([this](std::stop_token st) { threadMain(st); });
}

void MpClientSim::stop() {
	if (!thread_.has_value()) {
		return;
	}
	thread_->request_stop();
	thread_->join();
	thread_.reset();
}

void MpClientSim::syncJoin(const std::uint64_t joinPhysicsStep,
                           const double serverTimeScale,
                           std::vector<sim::AuthoritativeBody> bodies) {
	engine_.queueApplyAuthoritativeSnapshot(std::move(bodies));
	if (std::isfinite(serverTimeScale) && serverTimeScale > 0.0) {
		engine_.setTimeScale(serverTimeScale);
	}
	clientPhysicsHead_.store(joinPhysicsStep, std::memory_order_release);
	lastConfirmedAuthorityStep_.store(joinPhysicsStep, std::memory_order_release);
}

void MpClientSim::postMergeDeletes(std::vector<std::pair<sim::BodyId, sim::BodyId>> remaps) {
	if (remaps.empty()) {
		return;
	}
	std::lock_guard<std::mutex> lock(workMutex_);
	workQueue_.push_back([this, r = std::move(remaps)]() mutable {
		for (const auto& pr : r) {
			engine_.queueDelete(pr.first);
		}
	});
}

void MpClientSim::postAuthoritativeUpserts(std::vector<sim::AuthoritativeBody> bodies) {
	if (bodies.empty()) {
		return;
	}
	std::lock_guard<std::mutex> lock(workMutex_);
	workQueue_.push_back([this, b = std::move(bodies)]() mutable {
		engine_.queueUpsertAuthoritativeBodies(std::move(b));
	});
}

void MpClientSim::postDynamicsPatches(std::vector<sim::BodyDynamicsPatch> patches) {
	if (patches.empty()) {
		return;
	}
	std::lock_guard<std::mutex> lock(workMutex_);
	workQueue_.push_back([this, p = std::move(patches)]() mutable {
		engine_.queuePatchBodyDynamics(std::move(p));
	});
}

void MpClientSim::postAuthorityBundle(std::vector<sim::BodyDynamicsPatch> patches,
                                      const std::uint64_t latestAuthorityStep) {
	std::lock_guard<std::mutex> lock(workMutex_);
	workQueue_.push_back([this, p = std::move(patches), latestAuthorityStep]() mutable {
		if (!p.empty()) {
			engine_.queuePatchBodyDynamics(std::move(p));
		}
		lastConfirmedAuthorityStep_.store(latestAuthorityStep, std::memory_order_release);
	});
}

void MpClientSim::setLastConfirmedAuthorityStep(const std::uint64_t step) {
	std::lock_guard<std::mutex> lock(workMutex_);
	workQueue_.push_back([this, step]() {
		lastConfirmedAuthorityStep_.store(step, std::memory_order_release);
	});
}

void MpClientSim::syncReplicas(const std::unordered_map<sim::BodyId, MpShipReplicaInput>& replicas) {
	std::lock_guard<std::mutex> lock(replicaMutex_);
	replicas_ = replicas;
}

void MpClientSim::drainWorkQueue() {
	std::deque<std::function<void()>> batch;
	{
		std::lock_guard<std::mutex> lock(workMutex_);
		batch.swap(workQueue_);
	}
	for (std::function<void()>& fn : batch) {
		fn();
	}
}

void MpClientSim::forwardMergeEventsFromEngine() {
	std::vector<std::pair<sim::BodyId, sim::BodyId>> merges;
	engine_.drainMergeRemapEvents(merges);
	if (merges.empty()) {
		return;
	}
	std::lock_guard<std::mutex> lock(mergeOutMutex_);
	mergeOutQueue_.push_back(std::move(merges));
}

void MpClientSim::publishRenderStateFromEngine() {
	MpClientRenderPublish pub;
	engine_.copyBodies(pub.bodies);
	pub.config = engine_.config();
	pub.simulationTimeSeconds = engine_.simulationTimeSeconds();
	pub.simulatedSecondsPerUpdate = engine_.simulatedSecondsPerUpdate();
	pub.simulatedSecondsPerRealSecond = engine_.simulatedSecondsPerRealSecond();
	std::lock_guard<std::mutex> lock(renderMutex_);
	renderPublish_ = std::move(pub);
}

void MpClientSim::copyLatestRenderPublish(MpClientRenderPublish& out) const {
	std::lock_guard<std::mutex> lock(renderMutex_);
	out = renderPublish_;
}

void MpClientSim::takePendingMergeRemaps(std::vector<std::pair<sim::BodyId, sim::BodyId>>& out) {
	out.clear();
	std::deque<std::vector<std::pair<sim::BodyId, sim::BodyId>>> drained;
	{
		std::lock_guard<std::mutex> lock(mergeOutMutex_);
		drained.swap(mergeOutQueue_);
	}
	for (std::vector<std::pair<sim::BodyId, sim::BodyId>>& batch : drained) {
		for (std::pair<sim::BodyId, sim::BodyId>& pr : batch) {
			out.push_back(std::move(pr));
		}
	}
}

void MpClientSim::threadMain(const std::stop_token st) {
	using clock = std::chrono::steady_clock;
	auto last = clock::now();
	double wallAcc = 0.0;

	while (!st.stop_requested()) {
		const auto now = clock::now();
		double frameDt = std::chrono::duration<double>(now - last).count();
		last = now;
		if (!std::isfinite(frameDt) || frameDt <= 0.0) {
			frameDt = 0.0;
		}
		wallAcc += frameDt;
		wallAcc = std::min(wallAcc, net::kMaxWallPhysicsDebtSeconds);

		// Authority ordering: always apply any queued main-thread work (snapshots, deletes,
		// patches) before integrating the next physics step, so each `advanceFixedStep` sees a
		// consistent command batch at step boundary.
		drainWorkQueue();

		const sim::SimulationConfig physicsCfg = engine_.config();
		if (physicsCfg.paused) {
			publishRenderStateFromEngine();
			continue;
		}
		const double ts =
		    (std::isfinite(physicsCfg.timeScale) && physicsCfg.timeScale > 0.0) ? physicsCfg.timeScale
		                                                                         : 1.0;
		const double dtSim = net::simulationDtFromTimeScale(ts);

		int stepBudget = 0;
		while (stepBudget < net::kMaxCatchUpPhysicsStepsPerSimThreadWake) {
			drainWorkQueue();

			const std::uint64_t head = clientPhysicsHead_.load(std::memory_order_acquire);
			const std::uint64_t auth = lastConfirmedAuthorityStep_.load(std::memory_order_acquire);
			if (head >= auth + net::kMpMaxClientLeadPhysicsSteps) {
				break;
			}
			const std::uint64_t leadSteps =
			    (head > auth) ? (head - auth) : 0u;
			const double leadRatio =
			    std::clamp(static_cast<double>(leadSteps) /
			                   static_cast<double>(net::kMpMaxClientLeadPhysicsSteps),
			               0.0, 1.0);
			constexpr double kMinPaceScale = 0.15;
			const double paceScale = std::max(kMinPaceScale, 1.0 - leadRatio * leadRatio);
			const double requiredDebt = net::kRealSecondsPerPhysicsStep / paceScale;
			if (wallAcc < requiredDebt) {
				break;
			}

			std::unordered_map<sim::BodyId, MpShipReplicaInput> repCopy;
			{
				std::lock_guard<std::mutex> lock(replicaMutex_);
				repCopy = replicas_;
			}
			applyReplicaThrustImpl(engine_, repCopy);
			engine_.advanceFixedStep(dtSim, physicsCfg);
			clientPhysicsHead_.store(head + 1u, std::memory_order_release);
			wallAcc -= requiredDebt;
			++stepBudget;
			forwardMergeEventsFromEngine();
		}
		publishRenderStateFromEngine();
	}
}

}  // namespace net
