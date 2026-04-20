#pragma once

#include "sim/BodyId.hpp"
#include "sim/CommandQueue.hpp"

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace net {

constexpr std::uint32_t kMagic = 0x31475350u;  // 'P''G''S''1' little-endian
constexpr std::uint8_t kProtocolVersion = 3;

enum class MsgType : std::uint8_t {
	JoinRequest = 1,
	JoinAccept = 2,
	ClientInput = 3,
	ShipState = 4,
	WorldDynamicSnapshot = 5,
	MergeRemapBatch = 6,
};

#pragma pack(push, 1)
struct ClientInputPayload {
	std::uint32_t seq = 0;
	std::uint8_t thrustForward = 0;
	std::uint8_t thrustReverse = 0;
	float facingRadians = 0.f;
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
                    std::uint8_t thrustReverse,
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
                   std::uint8_t& thrustReverseOut);

bool writeWorldDynamicSnapshot(std::uint64_t serverTick,
                               std::uint64_t globalPhysicsStep,
                               const std::vector<sim::BodyId>& ids,
                               const double* px,
                               const double* py,
                               const double* vx,
                               const double* vy,
                               std::size_t n,
                               std::vector<std::uint8_t>& out);
bool readWorldDynamicSnapshot(const std::uint8_t* data,
                              std::size_t len,
                              std::uint64_t& tickOut,
                              std::uint64_t& globalPhysicsStepOut,
                              std::vector<sim::BodyId>& idsOut,
                              std::vector<double>& pxOut,
                              std::vector<double>& pyOut,
                              std::vector<double>& vxOut,
                              std::vector<double>& vyOut);

bool writeMergeRemapBatch(std::uint64_t serverTick,
                          const std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairs,
                          std::vector<std::uint8_t>& out);
bool readMergeRemapBatch(const std::uint8_t* data,
                         std::size_t len,
                         std::uint64_t& tickOut,
                         std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairsOut);

}  // namespace net
