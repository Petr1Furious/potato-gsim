#include <CLI11.hpp>
#include "net/MpConstants.hpp"
#include "net/Protocol.hpp"
#include "scenario/ScenarioManager.hpp"
#include "sim/SimulationEngine.hpp"

#include <enet/enet.h>

#include <algorithm>
#include <array>
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

/// Rolling mean uses this many completed server ticks (not wall seconds).
constexpr std::size_t kServerStressSampleTicks = 12;
/// Log when mean tick wall time exceeds this fraction of the nominal net tick period.
constexpr double kServerStressMeanTickSlowness = 1.10;
/// Log when physics debt exceeds this fraction of the engine clamp (falling behind on sim).
constexpr double kServerStressDebtFraction = 0.55;
/// Minimum server ticks between `[stress]` lines (avoids spam while overloaded).
constexpr std::uint64_t kServerStressLogMinTicksApart = 40;

struct ServerOptions {
	int port = kDefaultPort;
	double simTimeScale = 86400.0;
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
	double maxDeltaV = 30000.0;
	double deltaVRegenDelaySeconds = 5.0;
	/// Delta-v restored per **real-world** second while regen is active (after idle delay).
	double deltaVRegenPerRealSecond = 500.0;

	scenario::RandomPresetConfig randomCfg{};
};

struct ClientSlot {
	ENetPeer* peer = nullptr;
	std::uint64_t clientLogId = 0;
	std::string peerAddress;
	std::string shipName;
	net::ClientInputPayload lastInput{};
	sim::BodyId shipId = 0;
	bool hasShip = false;
	bool needJoinSnapshot = true;
	/// One-time broadcast so existing clients register this slot's ship in their IdIndexMap.
	bool shipAuthoritativeUpsertSent = false;
	double deltaVCurrent = 0.0;
	/// Wall-clock seconds since last effective thrust (not sim time; avoids instant regen at high
	/// `timeScale`).
	double wallSecondsSinceThrust = 0.0;
	std::uint8_t appliedThrustForward = 0;
	std::uint8_t appliedThrustPercent = 0;
	bool fireRequested = false;
	double shellCooldownWallSeconds = 0.0;
};

struct ActiveShell {
	sim::BodyId bodyId = 0;
	sim::BodyId ownerShipId = 0;
	std::string bodyName;
	double ageWallSeconds = 0.0;
};

std::string peerAddressString(const ENetPeer* peer) {
	if (peer == nullptr) {
		return "unknown";
	}
	std::array<char, 64> ip{};
	ip.fill('\0');
	if (enet_address_get_host_ip(&peer->address, ip.data(), ip.size()) != 0) {
		return std::string("?:") + std::to_string(peer->address.port);
	}
	return std::string(ip.data()) + ":" + std::to_string(peer->address.port);
}

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

const sim::BodySnapshot* findBodyById(const std::vector<sim::BodySnapshot>& bodies,
                                      const sim::BodyId id) {
	for (const sim::BodySnapshot& b : bodies) {
		if (b.id == id) {
			return &b;
		}
	}
	return nullptr;
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
	app.add_option("--max-delta-v", opts.maxDeltaV, "Max per-ship delta-v budget (m/s)")
	    ->capture_default_str();
	app.add_option("--delta-v-regen-delay", opts.deltaVRegenDelaySeconds,
	               "Real-world idle time before delta-v regeneration starts (s)")
	    ->capture_default_str();
	app.add_option("--delta-v-regen-rate", opts.deltaVRegenPerRealSecond,
	               "Delta-v restored per real-world second while regen is active (m/s per s)")
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
	opts.maxDeltaV = std::max(0.0, opts.maxDeltaV);
	opts.deltaVRegenDelaySeconds = std::max(0.0, opts.deltaVRegenDelaySeconds);
	opts.deltaVRegenPerRealSecond = std::max(0.0, opts.deltaVRegenPerRealSecond);

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
	std::vector<ActiveShell> activeShells;
	std::uint64_t nextShellSerial = 1;
	std::uint64_t nextClientLogId = 1;

	std::fprintf(stderr,
	             "potato_gsim_server *:%d | preset=%s | timeScale=%.6g | netTickHz=%d | "
	             "snapshotEvery=%llu ticks | maxClients=%d\n",
	             opts.port, presetName(opts.presetKind), opts.simTimeScale, opts.netTickHz,
	             static_cast<unsigned long long>(opts.worldSnapshotIntervalTicks), opts.maxClients);

	std::uint64_t serverTick = 0;
	std::uint64_t globalPhysicsStep = 0;
	std::size_t nextShipSpawnIndex = 0;
	auto lastTick = std::chrono::steady_clock::now();
	const double tickPeriod = 1.0 / static_cast<double>(opts.netTickHz);
	double serverWallPhysicsDebt = 0.0;
	std::array<double, kServerStressSampleTicks> stressTickWallSeconds{};
	std::array<int, kServerStressSampleTicks> stressPhysicsStepsPerTick{};
	std::size_t stressSampleCount = 0;
	std::size_t stressSampleIndex = 0;
	std::uint64_t packetsRxSinceStressLog = 0;
	std::uint64_t packetsTxSinceStressLog = 0;
	std::optional<std::uint64_t> lastStressLogTick;

	while (true) {
		ENetEvent event;
		while (enet_host_service(host, &event, 1) > 0) {
			if (event.type == ENET_EVENT_TYPE_CONNECT) {
				const std::string peerAddress = peerAddressString(event.peer);
				const std::uint64_t clientId = nextClientLogId++;
				sim::SpawnCommand shipSpawn = makeShipSpawn(nextShipSpawnIndex, opts);
				++nextShipSpawnIndex;
				const std::string shipName = shipSpawn.name;
				engine.queueSpawn(std::move(shipSpawn));
				ClientSlot slot;
				slot.peer = event.peer;
				slot.clientLogId = clientId;
				slot.peerAddress = peerAddress;
				slot.shipName = shipName;
				slot.deltaVCurrent = opts.maxDeltaV;
				// Must start at 0: priming to the delay makes regen begin the instant thrust stops.
				slot.wallSecondsSinceThrust = 0.0;
				clients.push_back(slot);
				std::fprintf(stderr, "[connect] client#%llu %s ship=%s peers=%zu/%d\n",
				             static_cast<unsigned long long>(clientId), peerAddress.c_str(),
				             shipName.c_str(), clients.size(), opts.maxClients);
			} else if (event.type == ENET_EVENT_TYPE_RECEIVE) {
				++packetsRxSinceStressLog;
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
									if (in.firePrimary != 0) {
										c.fireRequested = true;
									}
									break;
								}
							}
						}
					}
				}
				enet_packet_destroy(event.packet);
			} else if (event.type == ENET_EVENT_TYPE_DISCONNECT) {
				const std::string peerAddress = peerAddressString(event.peer);
				for (std::size_t i = 0; i < clients.size(); ++i) {
					if (clients[i].peer == event.peer) {
						const ClientSlot& c = clients[i];
						std::fprintf(
						    stderr,
						    "[disconnect] client#%llu from=%s shipName=%s shipId=%llu hadShip=%d\n",
						    static_cast<unsigned long long>(c.clientLogId), peerAddress.c_str(),
						    c.shipName.c_str(), static_cast<unsigned long long>(c.shipId),
						    c.hasShip ? 1 : 0);
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
		const double wallDtStep = net::kRealSecondsPerPhysicsStep;
		serverWallPhysicsDebt += elapsed;
		serverWallPhysicsDebt = std::min(serverWallPhysicsDebt, net::kMaxWallPhysicsDebtSeconds);

		int physicsSteps = 0;
		while (serverWallPhysicsDebt >= net::kRealSecondsPerPhysicsStep &&
		       physicsSteps < net::kMaxCatchUpPhysicsStepsPerServerTick) {
			for (ClientSlot& c : clients) {
				if (!c.hasShip || c.shipId == 0) {
					continue;
				}
				c.shellCooldownWallSeconds = std::max(0.0, c.shellCooldownWallSeconds - wallDtStep);
				const double f = static_cast<double>(c.lastInput.facingRadians);
				const double ca = std::cos(f);
				const double sa = std::sin(f);
				const double requestedThrustScale =
				    0.01 *
				    static_cast<double>(std::min<std::uint8_t>(c.lastInput.thrustPercent, 100));
				double ax = 0.0;
				double ay = 0.0;
				std::uint8_t effectiveThrustPercent = 0;
				std::uint8_t effectiveThrustForward = 0;
				if (c.lastInput.thrustForward) {
					const double requestedDeltaV =
					    net::kShipThrustAccel * requestedThrustScale * dtSim;
					const double allowedScale =
					    (requestedDeltaV > 1e-12)
					        ? std::clamp(c.deltaVCurrent / requestedDeltaV, 0.0, 1.0)
					        : 0.0;
					const double effectiveThrustScale = requestedThrustScale * allowedScale;
					const double consumedDeltaV = requestedDeltaV * allowedScale;
					c.deltaVCurrent = std::max(0.0, c.deltaVCurrent - consumedDeltaV);
					if (effectiveThrustScale > 1e-9) {
						effectiveThrustForward = 1;
						effectiveThrustPercent = static_cast<std::uint8_t>(
						    std::clamp(std::lround(effectiveThrustScale * 100.0), 0l, 100l));
						ax += net::kShipThrustAccel * effectiveThrustScale * ca;
						ay += net::kShipThrustAccel * effectiveThrustScale * sa;
					}
				}
				if (effectiveThrustForward) {
					c.wallSecondsSinceThrust = 0.0;
				} else {
					// Idle + regen use wall time per physics step so high `timeScale` does not
					// fast-forward the fuel clock (one sim step could otherwise exceed the delay).
					c.wallSecondsSinceThrust += wallDtStep;
					if (c.wallSecondsSinceThrust >= opts.deltaVRegenDelaySeconds) {
						c.deltaVCurrent =
						    std::min(opts.maxDeltaV,
						             c.deltaVCurrent + opts.deltaVRegenPerRealSecond * wallDtStep);
					}
				}
				c.appliedThrustForward = effectiveThrustForward;
				c.appliedThrustPercent = effectiveThrustPercent;
				engine.setShipThrustAccelWorld(c.shipId, ax, ay);
			}
			engine.advanceFixedStep(dtSim, liveCfg);
			++globalPhysicsStep;
			serverWallPhysicsDebt -= net::kRealSecondsPerPhysicsStep;
			++physicsSteps;
		}
		stressTickWallSeconds[stressSampleIndex] = elapsed;
		stressPhysicsStepsPerTick[stressSampleIndex] = physicsSteps;
		stressSampleIndex = (stressSampleIndex + 1) % kServerStressSampleTicks;
		stressSampleCount = std::min(stressSampleCount + 1, kServerStressSampleTicks);

		std::vector<sim::BodySnapshot> snaps;
		engine.copyBodies(snaps);

		// Resolve ship ids (and join accept) before shell handling so the first tick after connect
		// can fire; previously shells ran while `hasShip` was still false.
		for (ClientSlot& c : clients) {
			if (c.needJoinSnapshot) {
				if (const std::optional<sim::BodyId> shipId = findBodyIdByName(snaps, c.shipName)) {
					sendJoinAccept(c.peer, engine, serverTick, globalPhysicsStep, *shipId);
					++packetsTxSinceStressLog;
					c.needJoinSnapshot = false;
				}
			}
			if (!c.hasShip) {
				if (const std::optional<sim::BodyId> id = findBodyIdByName(snaps, c.shipName)) {
					c.shipId = *id;
					c.hasShip = true;
				}
			}
		}

		for (ClientSlot& c : clients) {
			if (!c.fireRequested || !c.hasShip || c.shipId == 0) {
				continue;
			}
			if (c.shellCooldownWallSeconds > 1e-9) {
				c.fireRequested = false;
				continue;
			}
			const sim::BodySnapshot* ship = findBodyById(snaps, c.shipId);
			if (ship == nullptr) {
				c.fireRequested = false;
				continue;
			}
			const double aim = static_cast<double>(c.lastInput.shellAimRadians);
			const double speedDesiredSim =
			    std::max(0.0, static_cast<double>(c.lastInput.shellExtraSpeed));
			const double launchSpeedSim =
			    std::clamp(speedDesiredSim, net::kShellSpeedMin, net::kShellSpeedMax);
			const double ca = std::cos(aim);
			const double sa = std::sin(aim);
			const double muzzleOffset =
			    ship->radius + net::kShellRadius + net::kShellMuzzleSurfaceGapWorld;
			const std::string shellName =
			    "shell/" + std::to_string(static_cast<unsigned long long>(nextShellSerial++));
			engine.queueSpawn(sim::SpawnCommand{
			    .x = ship->x + ca * muzzleOffset,
			    .y = ship->y + sa * muzzleOffset,
			    .vx = ship->vx + launchSpeedSim * ca,
			    .vy = ship->vy + launchSpeedSim * sa,
			    .mass = net::kShellMass,
			    .radius = net::kShellRadius,
			    .name = shellName,
			});
			activeShells.push_back(
			    ActiveShell{.bodyId = 0, .ownerShipId = c.shipId, .bodyName = shellName});
			c.shellCooldownWallSeconds = net::kShellCooldownRealSeconds;
			c.fireRequested = false;
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
				++packetsTxSinceStressLog;
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
				++packetsTxSinceStressLog;
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
					++packetsTxSinceStressLog;
					break;
				}
			}
		}

		const double advancedWallSeconds = static_cast<double>(physicsSteps) * wallDtStep;
		for (ActiveShell& s : activeShells) {
			s.ageWallSeconds += advancedWallSeconds;
			if (s.bodyId == 0) {
				if (const std::optional<sim::BodyId> sid = findBodyIdByName(snaps, s.bodyName)) {
					s.bodyId = *sid;
					if (const sim::BodySnapshot* shellBody = findBodyById(snaps, s.bodyId);
					    shellBody != nullptr) {
						std::vector<sim::AuthoritativeBody> one;
						one.push_back(sim::AuthoritativeBody{
						    .id = shellBody->id,
						    .x = shellBody->x,
						    .y = shellBody->y,
						    .vx = shellBody->vx,
						    .vy = shellBody->vy,
						    .mass = shellBody->mass,
						    .radius = shellBody->radius,
						    .name = shellBody->name,
						});
						std::vector<std::uint8_t> payload;
						net::writeAuthoritativeBodyUpsert(serverTick, globalPhysicsStep, one,
						                                  payload);
						ENetPacket* packet = enet_packet_create(payload.data(), payload.size(),
						                                        ENET_PACKET_FLAG_RELIABLE);
						enet_host_broadcast(host, 1, packet);
						++packetsTxSinceStressLog;
					}
				}
			}
		}
		std::vector<sim::BodyId> shellIdsToDelete;
		std::vector<sim::BodyId> shipIdsToDelete;
		for (const ActiveShell& s : activeShells) {
			if (s.bodyId == 0) {
				continue;
			}
			if (s.ageWallSeconds >= net::kShellLifetimeRealSeconds) {
				shellIdsToDelete.push_back(s.bodyId);
				continue;
			}
			const sim::BodySnapshot* shellBody = findBodyById(snaps, s.bodyId);
			if (shellBody == nullptr) {
				continue;
			}
			bool shouldExplode = false;
			for (const ClientSlot& c : clients) {
				if (!c.hasShip || c.shipId == 0) {
					continue;
				}
				if (c.shipId == s.ownerShipId &&
				    s.ageWallSeconds < net::kShellArmDelayRealSeconds) {
					continue;
				}
				const sim::BodySnapshot* shipBody = findBodyById(snaps, c.shipId);
				if (shipBody == nullptr) {
					continue;
				}
				const double dx = shipBody->x - shellBody->x;
				const double dy = shipBody->y - shellBody->y;
				const double rr = net::kShellExplosionRadius + shipBody->radius;
				if ((dx * dx + dy * dy) <= rr * rr) {
					shipIdsToDelete.push_back(c.shipId);
					shouldExplode = true;
				}
			}
			if (shouldExplode) {
				shellIdsToDelete.push_back(s.bodyId);
			}
		}
		if (!shipIdsToDelete.empty()) {
			std::sort(shipIdsToDelete.begin(), shipIdsToDelete.end());
			shipIdsToDelete.erase(std::unique(shipIdsToDelete.begin(), shipIdsToDelete.end()),
			                      shipIdsToDelete.end());
			for (const sim::BodyId id : shipIdsToDelete) {
				engine.queueDelete(id);
				for (ClientSlot& c : clients) {
					if (c.shipId == id) {
						std::fprintf(
						    stderr,
						    "[ship-destroyed] client#%llu shipId=%llu reason=shell-proximity\n",
						    static_cast<unsigned long long>(c.clientLogId),
						    static_cast<unsigned long long>(id));
						c.hasShip = false;
						c.shipId = 0;
						c.needJoinSnapshot = false;
						c.shipAuthoritativeUpsertSent = false;
					}
				}
			}
		}
		if (!shellIdsToDelete.empty()) {
			std::sort(shellIdsToDelete.begin(), shellIdsToDelete.end());
			shellIdsToDelete.erase(std::unique(shellIdsToDelete.begin(), shellIdsToDelete.end()),
			                       shellIdsToDelete.end());
			for (const sim::BodyId id : shellIdsToDelete) {
				engine.queueDelete(id);
			}
		}
		if (!shellIdsToDelete.empty()) {
			activeShells.erase(std::remove_if(activeShells.begin(), activeShells.end(),
			                                  [&](const ActiveShell& s) {
				                                  return s.bodyId != 0 &&
				                                         std::binary_search(
				                                             shellIdsToDelete.begin(),
				                                             shellIdsToDelete.end(), s.bodyId);
			                                  }),
			                   activeShells.end());
		}

		std::vector<sim::BodyId> netBodyDeletes;
		netBodyDeletes.reserve(shipIdsToDelete.size() + shellIdsToDelete.size());
		for (const sim::BodyId id : shipIdsToDelete) {
			netBodyDeletes.push_back(id);
		}
		for (const sim::BodyId id : shellIdsToDelete) {
			netBodyDeletes.push_back(id);
		}
		if (!netBodyDeletes.empty()) {
			std::sort(netBodyDeletes.begin(), netBodyDeletes.end());
			netBodyDeletes.erase(std::unique(netBodyDeletes.begin(), netBodyDeletes.end()),
			                     netBodyDeletes.end());
		}
		const std::unordered_set<sim::BodyId> omitFromSnapshotThisTick(netBodyDeletes.begin(),
		                                                               netBodyDeletes.end());
		if (!netBodyDeletes.empty()) {
			std::vector<std::uint8_t> delPayload;
			net::writeBodyDeleteBatch(serverTick, globalPhysicsStep, netBodyDeletes, delPayload);
			ENetPacket* delPacket =
			    enet_packet_create(delPayload.data(), delPayload.size(), ENET_PACKET_FLAG_RELIABLE);
			enet_host_broadcast(host, 1, delPacket);
			++packetsTxSinceStressLog;
		}

		for (ClientSlot& c : clients) {
			if (!c.hasShip) {
				continue;
			}
			for (const sim::BodySnapshot& b : snaps) {
				if (b.id != c.shipId) {
					continue;
				}
				std::uint64_t shellReadyStep = globalPhysicsStep;
				if (c.shellCooldownWallSeconds > 1e-9) {
					const double steps =
					    std::ceil(c.shellCooldownWallSeconds / net::kRealSecondsPerPhysicsStep);
					shellReadyStep += static_cast<std::uint64_t>(std::max(1.0, steps));
				}
				std::vector<std::uint8_t> payload;
				net::writeShipState(serverTick, globalPhysicsStep, b.id, b.x, b.y, b.vx, b.vy,
				                    c.lastInput.facingRadians, c.appliedThrustForward,
				                    c.appliedThrustPercent, static_cast<float>(c.deltaVCurrent),
				                    static_cast<float>(opts.maxDeltaV), shellReadyStep, payload);
				ENetPacket* packet =
				    enet_packet_create(payload.data(), payload.size(), ENET_PACKET_FLAG_RELIABLE);
				enet_host_broadcast(host, 0, packet);
				++packetsTxSinceStressLog;
				break;
			}
		}

		if ((serverTick % opts.worldSnapshotIntervalTicks) == 0) {
			std::vector<sim::AuthoritativeBody> snapBodies;
			snapBodies.reserve(snaps.size());
			for (const sim::BodySnapshot& b : snaps) {
				if (omitFromSnapshotThisTick.count(b.id) != 0) {
					continue;
				}
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
			++packetsTxSinceStressLog;
		}

		if (stressSampleCount >= kServerStressSampleTicks) {
			double sumWall = 0.0;
			double sumPhys = 0.0;
			for (std::size_t i = 0; i < kServerStressSampleTicks; ++i) {
				sumWall += stressTickWallSeconds[i];
				sumPhys += static_cast<double>(stressPhysicsStepsPerTick[i]);
			}
			const double meanWall = sumWall / static_cast<double>(kServerStressSampleTicks);
			const double meanPhysSteps = sumPhys / static_cast<double>(kServerStressSampleTicks);
			const double nominalPeriod = tickPeriod;
			const bool meanTooSlow = meanWall > nominalPeriod * kServerStressMeanTickSlowness;
			const bool physicsCapped = physicsSteps >= net::kMaxCatchUpPhysicsStepsPerServerTick;
			const double debtLimit = net::kMaxWallPhysicsDebtSeconds;
			const bool debtHigh =
			    debtLimit > 1e-12 && serverWallPhysicsDebt >= debtLimit * kServerStressDebtFraction;
			const bool stressed = meanTooSlow || physicsCapped || debtHigh;
			const bool cooldownOk =
			    !lastStressLogTick.has_value() ||
			    (serverTick - *lastStressLogTick) >= kServerStressLogMinTicksApart;
			if (stressed && cooldownOk) {
				const double tps = (meanWall > 1e-12) ? (1.0 / meanWall) : 0.0;
				const char* reason = meanTooSlow     ? "slow_mean"
				                     : physicsCapped ? "physics_cap"
				                                     : "debt";
				std::fprintf(stderr,
				             "[stress] tick=%llu reason=%s tps=%.2f avgSteps/tick=%.2f "
				             "debt=%.4fs rxPkts=%llu txPkts=%llu\n",
				             static_cast<unsigned long long>(serverTick), reason, tps,
				             meanPhysSteps, serverWallPhysicsDebt,
				             static_cast<unsigned long long>(packetsRxSinceStressLog),
				             static_cast<unsigned long long>(packetsTxSinceStressLog));
				lastStressLogTick = serverTick;
				packetsRxSinceStressLog = 0;
				packetsTxSinceStressLog = 0;
			}
		}

		enet_host_flush(host);
	}
}
