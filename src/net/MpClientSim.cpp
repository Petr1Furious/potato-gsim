#include "net/MpClientSim.hpp"
#include "net/MpConstants.hpp"

#include <algorithm>
#include <cassert>
#include <cmath>
#include <cstdio>

namespace net {

namespace {

void applyReplicaThrustImpl(sim::SimulationEngine& engine,
                            const std::unordered_map<sim::BodyId, MpShipReplicaInput>& replicas) {
	for (const auto& [id, rep] : replicas) {
		const double f = static_cast<double>(rep.facing);
		const double ca = std::cos(f);
		const double sa = std::sin(f);
		const double thrustScale =
		    0.01 * static_cast<double>(std::min<std::uint8_t>(rep.thrustPercent, 100));
		double ax = 0.0;
		double ay = 0.0;
		if (rep.thrustForward) {
			ax += net::kShipThrustAccel * thrustScale * ca;
			ay += net::kShipThrustAccel * thrustScale * sa;
		}
		engine.setShipThrustAccelWorld(id, ax, ay);
	}
}

const std::unordered_map<sim::BodyId, MpShipReplicaInput>* findReplayInputForStep(
    const std::deque<std::pair<std::uint64_t, std::unordered_map<sim::BodyId, MpShipReplicaInput>>>&
        history,
    const std::uint64_t step) {
	for (auto it = history.rbegin(); it != history.rend(); ++it) {
		if (it->first == step) {
			return &it->second;
		}
	}
	return nullptr;
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
	replayInputHistory_.clear();
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

void MpClientSim::postBodyDeleteBatch(std::vector<sim::BodyId> ids) {
	if (ids.empty()) {
		return;
	}
	std::lock_guard<std::mutex> lock(workMutex_);
	workQueue_.push_back([this, ids = std::move(ids)]() mutable {
		for (const sim::BodyId id : ids) {
			if (id != 0) {
				engine_.queueDelete(id);
			}
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
	workQueue_.push_back(
	    [this, p = std::move(patches)]() mutable { engine_.queuePatchBodyDynamics(std::move(p)); });
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
	workQueue_.push_back(
	    [this, step]() { lastConfirmedAuthorityStep_.store(step, std::memory_order_release); });
}

void MpClientSim::enqueueWorldSnapshot(WorldSnapshotJob job) {
	std::lock_guard<std::mutex> lock(snapshotJobMutex_);
	snapshotJobQueue_.push_back(std::move(job));
}

void MpClientSim::syncReplicas(
    const std::unordered_map<sim::BodyId, MpShipReplicaInput>& replicas) {
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

void MpClientSim::drainSnapshotJobQueue() {
	while (true) {
		WorldSnapshotJob job;
		{
			std::lock_guard<std::mutex> lock(snapshotJobMutex_);
			if (snapshotJobQueue_.empty()) {
				break;
			}
			job = std::move(snapshotJobQueue_.front());
			snapshotJobQueue_.pop_front();
		}
		processWorldSnapshotJob(job);
	}
}

void MpClientSim::processWorldSnapshotJob(const WorldSnapshotJob& job) {
	const std::uint64_t W = job.globalPhysicsStep;
	const std::uint64_t priorAuth = lastConfirmedAuthorityStep_.load(std::memory_order_acquire);
	if (W <= priorAuth) {
		return;
	}

	const std::uint64_t headBefore = clientPhysicsHead_.load(std::memory_order_acquire);

	std::vector<sim::BodySnapshot> preSnaps;
	engine_.copyBodies(preSnaps);
	std::vector<sim::AuthoritativeBody> preReplay;
	preReplay.reserve(preSnaps.size());
	for (const sim::BodySnapshot& s : preSnaps) {
		preReplay.push_back(sim::AuthoritativeBody{
		    .id = s.id,
		    .x = s.x,
		    .y = s.y,
		    .vx = s.vx,
		    .vy = s.vy,
		    .mass = s.mass,
		    .radius = s.radius,
		    .name = s.name,
		});
	}

	std::vector<sim::AuthoritativeBody> authoritativeAtW = job.bodies;
	engine_.applyAuthoritativeSnapshotImmediate(std::move(authoritativeAtW));

	if (W < headBefore) {
		clientPhysicsHead_.store(W, std::memory_order_release);
		const std::uint64_t nReplay = headBefore - W;
		int publishedSinceBatch = 0;
		for (std::uint64_t k = 0; k < nReplay; ++k) {
			const std::uint64_t s = W + k;
			const std::unordered_map<sim::BodyId, MpShipReplicaInput>* rep =
			    findReplayInputForStep(replayInputHistory_, s);
			if (rep == nullptr) {
				std::fprintf(stderr,
				             "MpClientSim: missing replay input for step %llu; restoring pre "
				             "snapshot\n",
				             static_cast<unsigned long long>(s));
				engine_.applyAuthoritativeSnapshotImmediate(std::move(preReplay));
				lastConfirmedAuthorityStep_.store(priorAuth, std::memory_order_release);
				clientPhysicsHead_.store(headBefore, std::memory_order_release);
				publishRenderStateFromEngine();
				return;
			}
			integrateOnePhysicsStep(*rep, false);
			++publishedSinceBatch;
			if (publishedSinceBatch >= net::kMaxCatchUpPhysicsStepsPerSimThreadWake) {
				publishRenderStateFromEngine();
				publishedSinceBatch = 0;
			}
		}
		if (publishedSinceBatch > 0) {
			publishRenderStateFromEngine();
		}
		const std::uint64_t headAfter = clientPhysicsHead_.load(std::memory_order_acquire);
#ifndef NDEBUG
		assert(headAfter == headBefore);
#endif
		if (headAfter != headBefore) {
			std::fprintf(stderr, "MpClientSim: replay head mismatch after=%llu expected=%llu\n",
			             static_cast<unsigned long long>(headAfter),
			             static_cast<unsigned long long>(headBefore));
		}
		lastConfirmedAuthorityStep_.store(W, std::memory_order_release);
	} else {
		lastConfirmedAuthorityStep_.store(W, std::memory_order_release);
	}
}

void MpClientSim::integrateOnePhysicsStep(
    const std::unordered_map<sim::BodyId, MpShipReplicaInput>& reps,
    const bool recordReplaySample) {
	if (recordReplaySample) {
		drainWorkQueue();
	}
	const sim::SimulationConfig physicsCfg = engine_.config();
	if (physicsCfg.paused) {
		return;
	}
	const double ts = (std::isfinite(physicsCfg.timeScale) && physicsCfg.timeScale > 0.0)
	                      ? physicsCfg.timeScale
	                      : 1.0;
	const double dtSim = net::simulationDtFromTimeScale(ts);

	if (recordReplaySample) {
		const std::uint64_t stepIdx = clientPhysicsHead_.load(std::memory_order_relaxed);
		while (replayInputHistory_.size() >= 512) {
			replayInputHistory_.pop_front();
		}
		replayInputHistory_.push_back({stepIdx, reps});
	}

	applyReplicaThrustImpl(engine_, reps);
	engine_.advanceFixedStep(dtSim, physicsCfg);
	clientPhysicsHead_.fetch_add(1u, std::memory_order_acq_rel);
	forwardMergeEventsFromEngine();
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

#if defined(POTATO_GSIM_MP_WORLD_SYNC_TESTS)
void MpClientSim::testingApplyQueuedNetworkWorkOnCallerThread() {
	assert(!thread_.has_value());
	drainWorkQueue();
	drainSnapshotJobQueue();
}
#endif

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

		drainWorkQueue();
		drainSnapshotJobQueue();

		const sim::SimulationConfig physicsCfg = engine_.config();
		if (physicsCfg.paused) {
			publishRenderStateFromEngine();
			continue;
		}

		int stepBudget = 0;
		while (stepBudget < net::kMaxCatchUpPhysicsStepsPerSimThreadWake) {
			drainWorkQueue();

			const std::uint64_t head = clientPhysicsHead_.load(std::memory_order_acquire);
			const std::uint64_t auth = lastConfirmedAuthorityStep_.load(std::memory_order_acquire);
			if (head >= auth + net::kMpMaxClientLeadPhysicsSteps) {
				break;
			}
			const std::uint64_t leadSteps = (head > auth) ? (head - auth) : 0u;
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
			integrateOnePhysicsStep(repCopy, true);
			wallAcc -= requiredDebt;
			++stepBudget;
		}
		publishRenderStateFromEngine();
	}
}

}  // namespace net
