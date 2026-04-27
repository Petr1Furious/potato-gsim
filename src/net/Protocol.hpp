#pragma once

#include "sim/BodyId.hpp"
#include "sim/CommandQueue.hpp"

#include <cstddef>
#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

namespace net {

constexpr std::uint32_t kMagic = 0x31475350u;  // 'P''G''S''1' little-endian
constexpr std::uint8_t kProtocolVersion = 1;

enum class MsgType : std::uint8_t {
	JoinRequest = 1,
	JoinAccept = 2,
	ClientInput = 3,
	ShipState = 4,
	WorldDynamicSnapshot = 5,
	MergeRemapBatch = 6,
	/// Reliable: add or update server bodies (e.g. another player ship after join).
	AuthoritativeBodyUpsert = 7,
	/// Reliable: remove bodies immediately (e.g. shell hit, shell expiry); same channel as upserts.
	BodyDeleteBatch = 8,
	/// Reliable: join denied; disconnect typically follows.
	JoinReject = 9,
	/// Reliable: respawn countdown for the receiving client (`respawnAtServerTick` = 0 clears).
	RespawnCountdown = 10,
};

/// Max UTF-8 bytes encoded in `writeJoinRequest` (after trim).
constexpr std::size_t kJoinRequestNameMaxBytes = 64;

enum class JoinRejectReason : std::uint8_t {
	ShipNameTaken = 0,
	NameInvalid = 1,
	ServerFull = 2,
};

#pragma pack(push, 1)
struct ClientInputPayload {
	std::uint32_t seq = 0;
	std::uint8_t thrustForward = 0;
	float facingRadians = 0.f;
	/// 0–100: scales forward thrust acceleration from this client.
	std::uint8_t thrustPercent = 100;
	/// Non-zero while the client requests primary fire for this input sample (server latches per
	/// tick).
	std::uint8_t firePrimary = 0;
	/// World-space launch direction for shell fire.
	float shellAimRadians = 0.f;
	/// Extra shell launch speed magnitude in **world units per simulated second** (client: ship→
	/// cursor distance ÷ `SimulationConfig::timeScale`). Server clamps to `kShellSpeedMin` /
	/// `kShellSpeedMax`.
	float shellExtraSpeed = 0.f;
};
#pragma pack(pop)

bool writeJoinRequest(std::string_view nameUtf8, std::vector<std::uint8_t>& out);
bool readJoinRequest(const std::uint8_t* data, std::size_t len, std::string& nameOut);

bool writeJoinReject(JoinRejectReason reason,
                     std::string_view detailUtf8,
                     std::vector<std::uint8_t>& out);
bool readJoinReject(const std::uint8_t* data,
                    std::size_t len,
                    JoinRejectReason& reasonOut,
                    std::string& detailOut);

bool writeJoinAccept(std::uint64_t serverTick,
                     std::uint64_t joinGlobalPhysicsStep,
                     const std::vector<sim::AuthoritativeBody>& bodies,
                     sim::BodyId ownShipBodyId,
                     double serverTimeScale,
                     double realSecondsPerPhysicsStep,
                     double shipThrustAccel,
                     std::vector<std::uint8_t>& out);
bool readJoinAccept(const std::uint8_t* data,
                    std::size_t len,
                    std::uint64_t& serverTickOut,
                    std::uint64_t& joinGlobalPhysicsStepOut,
                    std::vector<sim::AuthoritativeBody>& bodiesOut,
                    sim::BodyId& ownShipBodyIdOut,
                    double& serverTimeScaleOut,
                    double& realSecondsPerPhysicsStepOut,
                    double& shipThrustAccelOut);

bool writeClientInput(const ClientInputPayload& in, std::vector<std::uint8_t>& out);
bool readClientInput(const std::uint8_t* data, std::size_t len, ClientInputPayload& out);

struct ShipStateWire {
	sim::BodyId bodyId = 0;
	double px = 0.0;
	double py = 0.0;
	double vx = 0.0;
	double vy = 0.0;
	float facing = 0.0f;
	std::uint8_t thrustForward = 0;
	std::uint8_t thrustPercent = 100;
	float deltaVCurrentMps = 0.0f;
	float deltaVMaxMps = 0.0f;
	std::uint64_t shellReadyGlobalPhysicsStep = 0;
};

bool writeShipStateBatch(std::uint64_t serverTick,
                         std::uint64_t globalPhysicsStep,
                         const std::vector<ShipStateWire>& ships,
                         std::vector<std::uint8_t>& out);
bool readShipStateBatch(const std::uint8_t* data,
                        std::size_t len,
                        std::uint64_t& tickOut,
                        std::uint64_t& globalPhysicsStepOut,
                        std::vector<ShipStateWire>& shipsOut);

/// Full authoritative rows (same encoding as each `JoinAccept` body): id, pose, dynamics,
/// mass, radius, UTF-8 name.
bool writeWorldDynamicSnapshot(std::uint64_t serverTick,
                               std::uint64_t globalPhysicsStep,
                               const std::vector<sim::AuthoritativeBody>& bodies,
                               std::vector<std::uint8_t>& out);
bool readWorldDynamicSnapshot(const std::uint8_t* data,
                              std::size_t len,
                              std::uint64_t& tickOut,
                              std::uint64_t& globalPhysicsStepOut,
                              std::vector<sim::AuthoritativeBody>& bodiesOut);

bool writeMergeRemapBatch(std::uint64_t serverTick,
                          const std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairs,
                          std::vector<std::uint8_t>& out);
bool readMergeRemapBatch(const std::uint8_t* data,
                         std::size_t len,
                         std::uint64_t& tickOut,
                         std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairsOut);

bool writeAuthoritativeBodyUpsert(const std::uint64_t serverTick,
                                  const std::uint64_t globalPhysicsStep,
                                  const std::vector<sim::AuthoritativeBody>& bodies,
                                  std::vector<std::uint8_t>& out);
bool readAuthoritativeBodyUpsert(const std::uint8_t* data,
                                 std::size_t len,
                                 std::uint64_t& serverTickOut,
                                 std::uint64_t& globalPhysicsStepOut,
                                 std::vector<sim::AuthoritativeBody>& bodiesOut);

bool writeBodyDeleteBatch(std::uint64_t serverTick,
                          std::uint64_t globalPhysicsStep,
                          const std::vector<sim::BodyId>& ids,
                          std::vector<std::uint8_t>& out);
bool readBodyDeleteBatch(const std::uint8_t* data,
                         std::size_t len,
                         std::uint64_t& serverTickOut,
                         std::uint64_t& globalPhysicsStepOut,
                         std::vector<sim::BodyId>& idsOut);

bool writeRespawnCountdown(std::uint64_t serverTick,
                           std::uint64_t respawnAtServerTick,
                           double wallSecondsRemaining,
                           std::vector<std::uint8_t>& out);
bool readRespawnCountdown(const std::uint8_t* data,
                          std::size_t len,
                          std::uint64_t& serverTickOut,
                          std::uint64_t& respawnAtServerTickOut,
                          double& wallSecondsRemainingOut);

}  // namespace net
