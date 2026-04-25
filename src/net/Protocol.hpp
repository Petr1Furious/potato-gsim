#pragma once

#include "sim/BodyId.hpp"
#include "sim/CommandQueue.hpp"

#include <cstddef>
#include <cstdint>
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
};

#pragma pack(push, 1)
struct ClientInputPayload {
	std::uint32_t seq = 0;
	std::uint8_t thrustForward = 0;
	float facingRadians = 0.f;
	/// 0–100: scales forward thrust acceleration from this client.
	std::uint8_t thrustPercent = 100;
};
#pragma pack(pop)

bool writeJoinRequest(std::vector<std::uint8_t>& out);
bool readJoinRequest(const std::uint8_t* data, std::size_t len);

bool writeJoinAccept(std::uint64_t serverTick,
                     std::uint64_t joinGlobalPhysicsStep,
                     const std::vector<sim::AuthoritativeBody>& bodies,
                     sim::BodyId ownShipBodyId,
                     double serverTimeScale,
                     std::vector<std::uint8_t>& out);
bool readJoinAccept(const std::uint8_t* data,
                    std::size_t len,
                    std::uint64_t& serverTickOut,
                    std::uint64_t& joinGlobalPhysicsStepOut,
                    std::vector<sim::AuthoritativeBody>& bodiesOut,
                    sim::BodyId& ownShipBodyIdOut,
                    double& serverTimeScaleOut);

bool writeClientInput(const ClientInputPayload& in, std::vector<std::uint8_t>& out);
bool readClientInput(const std::uint8_t* data, std::size_t len, ClientInputPayload& out);

bool writeShipState(std::uint64_t serverTick,
                    std::uint64_t globalPhysicsStep,
                    sim::BodyId bodyId,
                    double px,
                    double py,
                    double vx,
                    double vy,
                    float facing,
                    std::uint8_t thrustForward,
                    std::uint8_t thrustPercent,
                    float deltaVCurrentMps,
                    float deltaVMaxMps,
                    std::vector<std::uint8_t>& out);
bool readShipState(const std::uint8_t* data,
                   std::size_t len,
                   std::uint64_t& tickOut,
                   std::uint64_t& globalPhysicsStepOut,
                   sim::BodyId& bodyIdOut,
                   double& px,
                   double& py,
                   double& vx,
                   double& vy,
                   float& facingOut,
                   std::uint8_t& thrustForwardOut,
                   std::uint8_t& thrustPercentOut,
                   float& deltaVCurrentMpsOut,
                   float& deltaVMaxMpsOut);

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

}  // namespace net
