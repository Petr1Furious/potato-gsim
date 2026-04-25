#include <CLI11.hpp>
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

struct ServerOptions {
	int port = kDefaultPort;
	double simTimeScale = 1.0;
	int netTickHz = 30;
	std::uint64_t worldSnapshotIntervalTicks = 30;
	int maxClients = 32;

	double gravitationalConstant = 6.67430e-11;
	double softeningEpsilon = 1.0e6;
	double barnesHutTheta = 0.6;
	double collisionCellScale = 8.0;
	int collisionStepInterval = 1;
	int workerCount = 0;
	std::size_t presetIndex = 0;
	scenario::PresetKind presetKind = scenario::PresetKind::Random;

	scenario::RandomPresetConfig randomCfg{};
};

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

std::optional<scenario::PresetKind> parsePreset(const std::string& raw) {
	const std::string s = raw;
	if (s == "solar" || s == "solar-like") {
		return scenario::PresetKind::SolarLike;
	}
	if (s == "binary" || s == "binary-dance") {
		return scenario::PresetKind::BinaryDance;
	}
	if (s == "spiral" || s == "spiral-cluster") {
		return scenario::PresetKind::SpiralCluster;
	}
	if (s == "random") {
		return scenario::PresetKind::Random;
	}
	return std::nullopt;
}

const char* presetName(const scenario::PresetKind p) {
	switch (p) {
		case scenario::PresetKind::SolarLike:
			return "solar";
		case scenario::PresetKind::BinaryDance:
			return "binary";
		case scenario::PresetKind::SpiralCluster:
			return "spiral";
		case scenario::PresetKind::Random:
			return "random";
	}
	return "random";
}

sim::SpawnCommand makeShipSpawn(const std::size_t idx, const ServerOptions& opts) {
	const double a = static_cast<double>(idx) * 2.399963229728653;  // Golden angle.
	double cx = 0.0;
	double cy = 0.0;
	double r = 5e10 + 5e9 * static_cast<double>(idx % 6);
	switch (opts.presetKind) {
		case scenario::PresetKind::SolarLike:
			r = 1.49598e11 + 1.5e9 * static_cast<double>(idx % 9);
			break;
		case scenario::PresetKind::BinaryDance:
			r = 4.2e11 + 2.2e9 * static_cast<double>(idx % 9);
			break;
		case scenario::PresetKind::SpiralCluster:
			r = 9.0e10 + 1.2e9 * static_cast<double>(idx % 10);
			break;
		case scenario::PresetKind::Random:
			cx = opts.randomCfg.centerX;
			cy = opts.randomCfg.centerY;
			r = std::max(1e8, opts.randomCfg.spreadRadius * 0.18) +
			    9e7 * static_cast<double>(idx % 16);
			break;
	}
	return sim::SpawnCommand{
	    .x = cx + r * std::cos(a),
	    .y = cy + r * std::sin(a),
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 2e4,
	    .radius = 15.0,
	    .name = "Ship" + std::to_string(static_cast<int>(idx)),
	};
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
	ServerOptions opts;
	opts.randomCfg = scenario::RandomPresetConfig{};
	std::string presetArg = "random";
	CLI::App app{"potato_gsim_server"};
	app.add_option("-p,--port", opts.port, "Server port")->capture_default_str();
	app.add_option("--time-scale", opts.simTimeScale, "Simulation seconds per real second")
	    ->capture_default_str();
	app.add_option("--net-tick-hz", opts.netTickHz, "Server net tick frequency (Hz)")
	    ->capture_default_str();
	app.add_option("--world-snapshot-interval", opts.worldSnapshotIntervalTicks,
	               "World snapshot packet interval in server ticks")
	    ->capture_default_str();
	app.add_option("--max-clients", opts.maxClients, "Maximum connected clients")
	    ->capture_default_str();
	app.add_option("--preset-index", opts.presetIndex, "0=solar, 1=binary, 2=spiral")
	    ->capture_default_str();
	app.add_option("--preset", presetArg, "Preset: solar|binary|spiral|random")
	    ->capture_default_str();

	app.add_option("--gravity", opts.gravitationalConstant, "Gravitational constant")
	    ->capture_default_str();
	app.add_option("--softening", opts.softeningEpsilon, "Softening epsilon")
	    ->capture_default_str();
	app.add_option("--theta", opts.barnesHutTheta, "Barnes-Hut theta")->capture_default_str();
	app.add_option("--collision-cell-scale", opts.collisionCellScale, "Collision cell scale")
	    ->capture_default_str();
	app.add_option("--collision-step-interval", opts.collisionStepInterval,
	               "Collision step interval")
	    ->capture_default_str();
	app.add_option("--workers", opts.workerCount, "Worker threads (0=auto/default)")
	    ->capture_default_str();

	app.add_option("--random-count", opts.randomCfg.count, "Random preset body count")
	    ->capture_default_str();
	app.add_option("--random-center-x", opts.randomCfg.centerX, "Random preset center X")
	    ->capture_default_str();
	app.add_option("--random-center-y", opts.randomCfg.centerY, "Random preset center Y")
	    ->capture_default_str();
	app.add_option("--random-spread", opts.randomCfg.spreadRadius, "Random preset spread radius")
	    ->capture_default_str();
	app.add_option("--random-mass-min", opts.randomCfg.massMin, "Random preset min mass")
	    ->capture_default_str();
	app.add_option("--random-mass-max", opts.randomCfg.massMax, "Random preset max mass")
	    ->capture_default_str();
	app.add_option("--random-jitter", opts.randomCfg.jitter, "Random preset spawn jitter")
	    ->capture_default_str();
	app.add_option("--random-tangent-scale", opts.randomCfg.tangentialVelocityScale,
	               "Random preset tangential speed scale")
	    ->capture_default_str();
	app.add_option("--random-seed", opts.randomCfg.seed, "Random preset deterministic seed");
	app.add_flag("--random-deterministic-seed", opts.randomCfg.useDeterministicSeed,
	             "Use --random-seed instead of std::random_device");
	CLI11_PARSE(app, argc, argv);

	if (const auto parsed = parsePreset(presetArg); parsed.has_value()) {
		opts.presetKind = *parsed;
	} else {
		opts.presetKind = scenario::ScenarioManager::presetKindFromIndex(opts.presetIndex);
	}
	if (!std::isfinite(opts.simTimeScale) || opts.simTimeScale <= 0.0) {
		opts.simTimeScale = 1.0;
	}
	opts.netTickHz = std::max(1, opts.netTickHz);
	opts.worldSnapshotIntervalTicks = std::max<std::uint64_t>(1, opts.worldSnapshotIntervalTicks);
	opts.maxClients = std::max(1, opts.maxClients);
	opts.collisionStepInterval = std::max(1, opts.collisionStepInterval);
	opts.workerCount = std::max(0, opts.workerCount);

	if (enet_initialize() != 0) {
		logErr("enet_initialize failed");
		return 1;
	}
	atexit(enet_deinitialize);

	ENetAddress address{};
	address.host = ENET_HOST_ANY;
	address.port = static_cast<std::uint16_t>(opts.port);

	ENetHost* host = enet_host_create(&address, static_cast<std::size_t>(opts.maxClients), 3, 0, 0);
	if (host == nullptr) {
		logErr("enet_host_create failed");
		return 1;
	}

	sim::SimulationConfig cfg;
	cfg.timeScale = opts.simTimeScale;
	cfg.fixedDtSeconds = net::kRealSecondsPerPhysicsStep;
	cfg.gravitationalConstant = opts.gravitationalConstant;
	cfg.softeningEpsilon = opts.softeningEpsilon;
	cfg.barnesHutTheta = opts.barnesHutTheta;
	cfg.collisionCellScale = opts.collisionCellScale;
	cfg.collisionStepInterval = opts.collisionStepInterval;
	cfg.workerCount = opts.workerCount;
	sim::SimulationEngine engine(cfg);
	// Drive stepping from this thread only (no background simulationLoop).
	std::vector<sim::SpawnCommand> initialBodies;
	if (opts.presetKind == scenario::PresetKind::Random) {
		initialBodies = scenario::ScenarioManager::makeRandom(opts.randomCfg);
	} else {
		initialBodies = scenario::ScenarioManager::makePreset(opts.presetKind);
	}
	engine.queueReplaceWorld(std::move(initialBodies));

	std::vector<ClientSlot> clients;

	std::fprintf(stderr,
	             "potato_gsim_server *:%d | preset=%s | timeScale=%.6g | netTickHz=%d | "
	             "snapshotEvery=%llu ticks | maxClients=%d\n",
	             opts.port, presetName(opts.presetKind), opts.simTimeScale, opts.netTickHz,
	             static_cast<unsigned long long>(opts.worldSnapshotIntervalTicks), opts.maxClients);

	std::uint64_t serverTick = 0;
	std::uint64_t globalPhysicsStep = 0;
	auto lastTick = std::chrono::steady_clock::now();
	const double tickPeriod = 1.0 / static_cast<double>(opts.netTickHz);
	double serverWallPhysicsDebt = 0.0;

	while (true) {
		ENetEvent event;
		while (enet_host_service(host, &event, 1) > 0) {
			if (event.type == ENET_EVENT_TYPE_CONNECT) {
				std::fprintf(stderr, "client connected\n");
				sim::SpawnCommand shipSpawn = makeShipSpawn(clients.size(), opts);
				const std::string shipName = shipSpawn.name;
				engine.queueSpawn(std::move(shipSpawn));
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

		if ((serverTick % opts.worldSnapshotIntervalTicks) == 0) {
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
