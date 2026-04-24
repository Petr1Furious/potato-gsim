#include "net/MpConstants.hpp"
#include "net/Protocol.hpp"
#include "scenario/ScenarioManager.hpp"
#include "sim/SimulationEngine.hpp"

#include <enet/enet.h>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <optional>
#include <string>
#include <unordered_set>
#include <vector>

namespace {

constexpr int kDefaultPort = 27777;
constexpr int kNetTickHz = 30;
constexpr std::uint64_t kWorldSnapshotIntervalTicks = 30;

struct ClientSlot {
	ENetPeer* peer = nullptr;
	std::string shipName;
	net::ClientInputPayload lastInput{};
	sim::BodyId shipId = 0;
	bool hasShip = false;
	bool needJoinSnapshot = true;
	bool loggedShipId = false;
	/// One-time broadcast so existing clients register this slot's ship in their IdIndexMap.
	bool shipAuthoritativeUpsertSent = false;
};

void logErr(const char* msg) {
	std::fprintf(stderr, "%s\n", msg);
}

std::optional<sim::BodyId> findBodyIdByName(const std::vector<sim::BodySnapshot>& bodies,
                                            const std::string& name) {
	for (const sim::BodySnapshot& b : bodies) {
		if (b.name == name) {
			return b.id;
		}
	}
	return std::nullopt;
}

void sendJoinAccept(ENetPeer* peer,
                    sim::SimulationEngine& engine,
                    const std::uint64_t serverTick,
                    const std::uint64_t joinGlobalPhysicsStep,
                    const sim::BodyId ownShipBodyId) {
	std::vector<sim::BodySnapshot> snaps;
	engine.copyBodies(snaps);
	std::vector<sim::AuthoritativeBody> bodies;
	bodies.reserve(snaps.size());
	for (const sim::BodySnapshot& s : snaps) {
		bodies.push_back(sim::AuthoritativeBody{
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
	std::vector<std::uint8_t> payload;
	const double ts = engine.config().timeScale;
	net::writeJoinAccept(serverTick, joinGlobalPhysicsStep, bodies, ownShipBodyId, ts, payload);
	ENetPacket* packet =
	    enet_packet_create(payload.data(), payload.size(), ENET_PACKET_FLAG_RELIABLE);
	enet_peer_send(peer, 2, packet);
}

}  // namespace

int main(int argc, char** argv) {
	int port = kDefaultPort;
	double simTimeScale = 1.0;
	if (argc >= 2) {
		port = std::atoi(argv[1]);
	}
	if (argc >= 3) {
		simTimeScale = std::strtod(argv[2], nullptr);
	}
	if (!std::isfinite(simTimeScale) || simTimeScale <= 0.0) {
		simTimeScale = 1.0;
	}

	if (enet_initialize() != 0) {
		logErr("enet_initialize failed");
		return 1;
	}
	atexit(enet_deinitialize);

	ENetAddress address{};
	address.host = ENET_HOST_ANY;
	address.port = static_cast<std::uint16_t>(port);

	ENetHost* host = enet_host_create(&address, 32, 3, 0, 0);
	if (host == nullptr) {
		logErr("enet_host_create failed");
		return 1;
	}

	sim::SimulationConfig cfg;
	cfg.timeScale = simTimeScale;
	cfg.fixedDtSeconds = net::kRealSecondsPerPhysicsStep;
	cfg.gravitationalConstant = 6.67430e-11;
	cfg.softeningEpsilon = 1.0e6;
	cfg.barnesHutTheta = 0.6;
	cfg.collisionCellScale = 8.0;
	cfg.collisionStepInterval = 1;
	cfg.workerCount = 0;
	sim::SimulationEngine engine(cfg);
	// Drive stepping from this thread only (no background simulationLoop).
	// engine.queueReplaceWorld(scenario::ScenarioManager::makePreset(0));
	engine.queueReplaceWorld(scenario::ScenarioManager::makeRandom(1000, 0.0, 0.0, 1e11));

	std::vector<ClientSlot> clients;

	std::fprintf(stderr,
	             "potato_gsim_server *:%d | timeScale=%.6g sim s/real s | 240 phys ticks/s wall, "
	             "dt_sim=timeScale/240 (argv: port [timeScale])\n",
	             port, simTimeScale);

	std::uint64_t serverTick = 0;
	std::uint64_t globalPhysicsStep = 0;
	auto lastTick = std::chrono::steady_clock::now();
	const double tickPeriod = 1.0 / static_cast<double>(kNetTickHz);
	double serverWallPhysicsDebt = 0.0;

	while (true) {
		ENetEvent event;
		while (enet_host_service(host, &event, 1) > 0) {
			if (event.type == ENET_EVENT_TYPE_CONNECT) {
				std::fprintf(stderr, "client connected\n");
				const std::string shipName =
				    "Ship" + std::to_string(static_cast<int>(clients.size()));
				engine.queueSpawn(sim::SpawnCommand{
				    .x = 5e10,
				    .y = 0.0,
				    .vx = 0.0,
				    .vy = 0,
				    .mass = 2e4,
				    .radius = 15.0,
				    .name = shipName,
				});
				ClientSlot slot;
				slot.peer = event.peer;
				slot.shipName = shipName;
				clients.push_back(slot);
			} else if (event.type == ENET_EVENT_TYPE_RECEIVE) {
				const std::uint8_t* d = event.packet->data;
				const std::size_t len = event.packet->dataLength;
				if (len >= 6 && d[4] == net::kProtocolVersion) {
					const auto type = static_cast<net::MsgType>(d[5]);
					if (type == net::MsgType::ClientInput) {
						net::ClientInputPayload in{};
						if (net::readClientInput(d, len, in)) {
							for (ClientSlot& c : clients) {
								if (c.peer == event.peer) {
									c.lastInput = in;
									break;
								}
							}
						}
					}
				}
				enet_packet_destroy(event.packet);
			} else if (event.type == ENET_EVENT_TYPE_DISCONNECT) {
				std::fprintf(stderr, "client disconnected\n");
				for (std::size_t i = 0; i < clients.size(); ++i) {
					if (clients[i].peer == event.peer) {
						clients.erase(clients.begin() + static_cast<std::ptrdiff_t>(i));
						break;
					}
				}
			}
		}

		const auto now = std::chrono::steady_clock::now();
		const double elapsed = std::chrono::duration<double>(now - lastTick).count();
		if (elapsed < tickPeriod) {
			continue;
		}
		lastTick = now;
		++serverTick;

		const sim::SimulationConfig liveCfg = engine.config();
		const double ts =
		    (std::isfinite(liveCfg.timeScale) && liveCfg.timeScale > 0.0) ? liveCfg.timeScale : 1.0;
		const double dtSim = net::simulationDtFromTimeScale(ts);
		serverWallPhysicsDebt += elapsed;
		serverWallPhysicsDebt = std::min(serverWallPhysicsDebt, net::kMaxWallPhysicsDebtSeconds);

		int physicsSteps = 0;
		while (serverWallPhysicsDebt >= net::kRealSecondsPerPhysicsStep &&
		       physicsSteps < net::kMaxCatchUpPhysicsStepsPerServerTick) {
			for (ClientSlot& c : clients) {
				if (!c.hasShip || c.shipId == 0) {
					continue;
				}
				const double f = static_cast<double>(c.lastInput.facingRadians);
				const double ca = std::cos(f);
				const double sa = std::sin(f);
				const double thrustScale = 0.01 * static_cast<double>(std::min<std::uint8_t>(
				                                      c.lastInput.thrustPercent, 100));
				double ax = 0.0;
				double ay = 0.0;
				if (c.lastInput.thrustForward) {
					ax += net::kShipThrustAccel * thrustScale * ca;
					ay += net::kShipThrustAccel * thrustScale * sa;
				}
				engine.setShipThrustAccelWorld(c.shipId, ax, ay);
			}
			engine.advanceFixedStep(dtSim, liveCfg);
			++globalPhysicsStep;
			serverWallPhysicsDebt -= net::kRealSecondsPerPhysicsStep;
			++physicsSteps;
		}

		std::vector<sim::BodySnapshot> snaps;
		engine.copyBodies(snaps);

		for (ClientSlot& c : clients) {
			if (c.needJoinSnapshot) {
				if (const std::optional<sim::BodyId> shipId = findBodyIdByName(snaps, c.shipName)) {
					sendJoinAccept(c.peer, engine, serverTick, globalPhysicsStep, *shipId);
					c.needJoinSnapshot = false;
				}
			}
			if (!c.hasShip) {
				if (const std::optional<sim::BodyId> id = findBodyIdByName(snaps, c.shipName)) {
					c.shipId = *id;
					c.hasShip = true;
					if (!c.loggedShipId) {
						std::fprintf(stderr, "assigned %s id=%llu\n", c.shipName.c_str(),
						             static_cast<unsigned long long>(c.shipId));
						c.loggedShipId = true;
					}
				}
			}
		}

		for (ClientSlot& c : clients) {
			if (!c.hasShip || c.shipId == 0 || c.shipAuthoritativeUpsertSent) {
				continue;
			}
			for (const sim::BodySnapshot& b : snaps) {
				if (b.id != c.shipId) {
					continue;
				}
				std::vector<sim::AuthoritativeBody> one;
				one.push_back(sim::AuthoritativeBody{
				    .id = b.id,
				    .x = b.x,
				    .y = b.y,
				    .vx = b.vx,
				    .vy = b.vy,
				    .mass = b.mass,
				    .radius = b.radius,
				    .name = b.name,
				});
				std::vector<std::uint8_t> payload;
				net::writeAuthoritativeBodyUpsert(serverTick, globalPhysicsStep, one, payload);
				ENetPacket* packet =
				    enet_packet_create(payload.data(), payload.size(), ENET_PACKET_FLAG_RELIABLE);
				enet_host_broadcast(host, 1, packet);
				c.shipAuthoritativeUpsertSent = true;
				break;
			}
		}

		std::vector<std::pair<sim::BodyId, sim::BodyId>> mergesThisTick;
		{
			engine.drainMergeRemapEvents(mergesThisTick);
			if (!mergesThisTick.empty()) {
				std::vector<std::uint8_t> payload;
				net::writeMergeRemapBatch(serverTick, mergesThisTick, payload);
				ENetPacket* packet =
				    enet_packet_create(payload.data(), payload.size(), ENET_PACKET_FLAG_RELIABLE);
				enet_host_broadcast(host, 1, packet);
			}
		}

		engine.copyBodies(snaps);
		if (!mergesThisTick.empty()) {
			std::unordered_set<sim::BodyId> survivorIds;
			survivorIds.reserve(mergesThisTick.size());
			for (const std::pair<sim::BodyId, sim::BodyId>& pr : mergesThisTick) {
				survivorIds.insert(pr.second);
			}
			for (const sim::BodyId survivorId : survivorIds) {
				for (const sim::BodySnapshot& b : snaps) {
					if (b.id != survivorId) {
						continue;
					}
					std::vector<sim::AuthoritativeBody> one;
					one.push_back(sim::AuthoritativeBody{
					    .id = b.id,
					    .x = b.x,
					    .y = b.y,
					    .vx = b.vx,
					    .vy = b.vy,
					    .mass = b.mass,
					    .radius = b.radius,
					    .name = b.name,
					});
					std::vector<std::uint8_t> payload;
					net::writeAuthoritativeBodyUpsert(serverTick, globalPhysicsStep, one, payload);
					ENetPacket* packet = enet_packet_create(payload.data(), payload.size(),
					                                        ENET_PACKET_FLAG_RELIABLE);
					enet_host_broadcast(host, 1, packet);
					break;
				}
			}
		}
		for (ClientSlot& c : clients) {
			if (!c.hasShip) {
				continue;
			}
			for (const sim::BodySnapshot& b : snaps) {
				if (b.id != c.shipId) {
					continue;
				}
				std::vector<std::uint8_t> payload;
				net::writeShipState(serverTick, globalPhysicsStep, b.id, b.x, b.y, b.vx, b.vy,
				                    c.lastInput.facingRadians, c.lastInput.thrustForward,
				                    c.lastInput.thrustPercent, payload);
				ENetPacket* packet =
				    enet_packet_create(payload.data(), payload.size(), ENET_PACKET_FLAG_RELIABLE);
				enet_host_broadcast(host, 0, packet);
				break;
			}
		}

		if ((serverTick % kWorldSnapshotIntervalTicks) == 0) {
			std::vector<sim::AuthoritativeBody> snapBodies;
			snapBodies.reserve(snaps.size());
			for (const sim::BodySnapshot& b : snaps) {
				snapBodies.push_back(sim::AuthoritativeBody{
				    .id = b.id,
				    .x = b.x,
				    .y = b.y,
				    .vx = b.vx,
				    .vy = b.vy,
				    .mass = b.mass,
				    .radius = b.radius,
				    .name = b.name,
				});
			}
			std::vector<std::uint8_t> payload;
			net::writeWorldDynamicSnapshot(serverTick, globalPhysicsStep, snapBodies, payload);
			ENetPacket* packet =
			    enet_packet_create(payload.data(), payload.size(), ENET_PACKET_FLAG_RELIABLE);
			enet_host_broadcast(host, 2, packet);
		}

		enet_host_flush(host);
	}
}
