#include "net/MpClient.hpp"
#include "net/MpClientSim.hpp"
#include "net/MpConstants.hpp"
#include "net/Protocol.hpp"
#include "sim/CommandQueue.hpp"
#include "sim/SimulationEngine.hpp"

#include <cassert>
#include <chrono>
#include <cstdint>
#include <thread>
#include <utility>
#include <vector>

namespace {

sim::SimulationConfig testPhysicsConfig() {
	sim::SimulationConfig cfg{};
	cfg.timeScale = 1.0;
	cfg.fixedDtSeconds = net::kRealSecondsPerPhysicsStep;
	cfg.paused = false;
	cfg.gravitationalConstant = 0.0;
	cfg.workerCount = 1;
	cfg.parallelChunkSize = 0;
	cfg.directSerialMaxBodies = 64;
	cfg.directParallelMaxBodies = 64;
	return cfg;
}

void test_merge_fifo_and_world_fifo() {
	net::MpClient client;
	std::vector<std::uint8_t> a;
	std::vector<std::uint8_t> b;
	assert(net::writeMergeRemapBatch(10, {{1, 2}}, a));
	assert(net::writeMergeRemapBatch(11, {{3, 4}}, b));
	client.testingEnqueueInboundPacket(std::move(a));
	client.testingEnqueueInboundPacket(std::move(b));
	client.service(0);
	std::uint64_t t1 = 0;
	std::vector<std::pair<sim::BodyId, sim::BodyId>> m1;
	assert(client.takeNextMergeRemaps(t1, m1));
	assert(t1 == 10 && m1.size() == 1 && m1[0].first == 1 && m1[0].second == 2);
	std::uint64_t t2 = 0;
	std::vector<std::pair<sim::BodyId, sim::BodyId>> m2;
	assert(client.takeNextMergeRemaps(t2, m2));
	assert(t2 == 11 && m2.size() == 1 && m2[0].first == 3 && m2[0].second == 4);
	std::uint64_t dummy = 0;
	std::vector<std::pair<sim::BodyId, sim::BodyId>> empty;
	assert(!client.takeNextMergeRemaps(dummy, empty));

	const sim::BodyId id = 99;
	std::vector<std::uint8_t> w1;
	std::vector<std::uint8_t> w2;
	assert(net::writeWorldDynamicSnapshot(
	    100, 1000,
	    std::vector<sim::AuthoritativeBody>{sim::AuthoritativeBody{.id = id,
	                                                               .x = 1.0,
	                                                               .y = 2.0,
	                                                               .vx = 3.0,
	                                                               .vy = 4.0,
	                                                               .mass = 10.0,
	                                                               .radius = 0.5,
	                                                               .name = "n"}},
	    w1));
	assert(net::writeWorldDynamicSnapshot(
	    101, 1001,
	    std::vector<sim::AuthoritativeBody>{sim::AuthoritativeBody{.id = id,
	                                                               .x = 5.0,
	                                                               .y = 2.0,
	                                                               .vx = 3.0,
	                                                               .vy = 4.0,
	                                                               .mass = 10.0,
	                                                               .radius = 0.5,
	                                                               .name = "n"}},
	    w2));
	client.testingEnqueueInboundPacket(std::move(w1));
	client.testingEnqueueInboundPacket(std::move(w2));
	client.service(0);
	std::uint64_t wt1 = 0;
	std::uint64_t ws1 = 0;
	std::vector<sim::AuthoritativeBody> bodies1;
	assert(client.takeNextWorldSnapshot(wt1, ws1, bodies1));
	assert(wt1 == 100 && ws1 == 1000 && bodies1.size() == 1 && bodies1[0].x == 1.0);
	assert(std::abs(bodies1[0].mass - 10.0) < 1e-12);
	std::uint64_t wt2 = 0;
	std::uint64_t ws2 = 0;
	std::vector<sim::AuthoritativeBody> bodies2;
	assert(client.takeNextWorldSnapshot(wt2, ws2, bodies2));
	assert(wt2 == 101 && ws2 == 1001 && bodies2[0].x == 5.0);
	assert(!client.takeNextWorldSnapshot(wt2, ws2, bodies2));
}

void test_stale_world_snapshot_dropped_on_sim() {
	sim::SimulationEngine engine(testPhysicsConfig());
	net::MpClientSim mp(engine);
	const sim::BodyId sunId = 9001;
	std::vector<sim::AuthoritativeBody> join;
	join.push_back(sim::AuthoritativeBody{
	    .id = sunId,
	    .x = 0.0,
	    .y = 0.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 1.0e12,
	    .radius = 1.0e6,
	    .name = "sun",
	});
	mp.syncJoin(50, 1.0, std::move(join));
	mp.start();
	std::this_thread::sleep_for(std::chrono::milliseconds(80));
	net::WorldSnapshotJob late;
	late.serverTick = 1;
	late.globalPhysicsStep = 40;
	late.bodies.push_back(sim::AuthoritativeBody{
	    .id = sunId,
	    .x = 0.0,
	    .y = 0.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 1.0e12,
	    .radius = 1.0e6,
	    .name = "sun",
	});
	mp.enqueueWorldSnapshot(std::move(late));
	std::this_thread::sleep_for(std::chrono::milliseconds(80));
	assert(mp.lastConfirmedAuthorityStep() == 50);
	mp.stop();
}

void test_replay_restores_physics_head() {
	sim::SimulationEngine engine(testPhysicsConfig());
	net::MpClientSim mp(engine);
	const sim::BodyId sunId = 42;
	std::vector<sim::AuthoritativeBody> join;
	join.push_back(sim::AuthoritativeBody{
	    .id = sunId,
	    .x = 0.0,
	    .y = 0.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 1.0e12,
	    .radius = 1.0e6,
	    .name = "sun",
	});
	mp.syncJoin(0, 1.0, std::move(join));
	mp.start();
	std::this_thread::sleep_for(std::chrono::milliseconds(400));
	mp.stop();
	const std::uint64_t headBefore = mp.clientPhysicsHead();
	assert(headBefore >= 30);
	net::WorldSnapshotJob job;
	job.serverTick = 7;
	job.globalPhysicsStep = 10;
	job.bodies.push_back(sim::AuthoritativeBody{
	    .id = sunId,
	    .x = 0.0,
	    .y = 0.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 1.0e12,
	    .radius = 1.0e6,
	    .name = "sun",
	});
	mp.enqueueWorldSnapshot(std::move(job));
	mp.testingApplyQueuedNetworkWorkOnCallerThread();
	assert(mp.clientPhysicsHead() == headBefore);
	assert(mp.lastConfirmedAuthorityStep() >= 10);
}

void test_authoritative_upsert_before_snapshot_mass() {
	sim::SimulationEngine engine(testPhysicsConfig());
	net::MpClientSim mp(engine);
	const sim::BodyId id = 7;
	std::vector<sim::AuthoritativeBody> join;
	join.push_back(sim::AuthoritativeBody{
	    .id = id,
	    .x = 1.0,
	    .y = 2.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 10.0,
	    .radius = 3.0,
	    .name = "a",
	});
	mp.syncJoin(0, 1.0, std::move(join));
	mp.start();
	std::this_thread::sleep_for(std::chrono::milliseconds(60));
	std::vector<sim::AuthoritativeBody> up;
	up.push_back(sim::AuthoritativeBody{
	    .id = id,
	    .x = 1.0,
	    .y = 2.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 77.0,
	    .radius = 3.0,
	    .name = "a",
	});
	mp.postAuthoritativeUpserts(std::move(up));
	std::this_thread::sleep_for(std::chrono::milliseconds(60));
	const std::uint64_t auth = mp.lastConfirmedAuthorityStep();
	net::WorldSnapshotJob job;
	job.serverTick = 9;
	job.globalPhysicsStep = auth + 1;
	job.bodies.push_back(sim::AuthoritativeBody{
	    .id = id,
	    .x = 1.0,
	    .y = 2.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 77.0,
	    .radius = 3.0,
	    .name = "a",
	});
	mp.enqueueWorldSnapshot(std::move(job));
	std::this_thread::sleep_for(std::chrono::milliseconds(120));
	std::vector<sim::BodySnapshot> snaps;
	engine.copyBodies(snaps);
	assert(snaps.size() == 1);
	assert(snaps[0].id == id);
	assert(std::abs(snaps[0].mass - 77.0) < 1e-6);
	mp.stop();
}

}  // namespace

int main() {
	test_merge_fifo_and_world_fifo();
	test_stale_world_snapshot_dropped_on_sim();
	test_replay_restores_physics_head();
	test_authoritative_upsert_before_snapshot_mass();
	return 0;
}
