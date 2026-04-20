#pragma once

#include "net/Protocol.hpp"
#include "sim/BodyId.hpp"
#include "sim/CommandQueue.hpp"

#include <enet/enet.h>

#include <cstdint>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace net {

/// Thin ENet client for potato_gsim multiplayer v0 (single server peer).
class MpClient {
   public:
	struct ShipNetSample {
		std::uint64_t serverTick = 0;
		std::uint64_t globalPhysicsStep = 0;
		sim::BodyId bodyId = 0;
		double px = 0.0;
		double py = 0.0;
		double vx = 0.0;
		double vy = 0.0;
		float facingRadians = 0.f;
		std::uint8_t thrustForward = 0;
		std::uint8_t thrustReverse = 0;
	};

	MpClient() = default;
	MpClient(const MpClient&) = delete;
	MpClient& operator=(const MpClient&) = delete;
	MpClient(MpClient&&) = delete;
	MpClient& operator=(MpClient&&) = delete;
	~MpClient();

	[[nodiscard]] bool connect(const std::string& host, std::uint16_t port);
	void disconnect();

	/// Poll network; timeout in ms (0 = non-blocking).
	void service(int timeoutMs);

	void sendJoinRequest();
	void sendInput(const ClientInputPayload& payload);

	[[nodiscard]] bool isConnected() const { return serverPeer_ != nullptr; }
	/// True once the outgoing connection handshake has completed (safe to send join).
	[[nodiscard]] bool isPeerConnected() const {
		return serverPeer_ != nullptr && serverPeer_->state == ENET_PEER_STATE_CONNECTED;
	}

	void takeJoinAccept(std::uint64_t& tickOut,
	                    std::uint64_t& joinGlobalPhysicsStepOut,
	                    std::vector<sim::AuthoritativeBody>& bodiesOut,
	                    sim::BodyId& ownShipBodyIdOut,
	                    double& serverTimeScaleOut);
	void takeShipSamples(std::vector<ShipNetSample>& out);
	void takeWorldSnapshot(std::uint64_t& tickOut,
	                       std::uint64_t& globalPhysicsStepOut,
	                       std::vector<sim::BodyId>& idsOut,
	                       std::vector<double>& pxOut,
	                       std::vector<double>& pyOut,
	                       std::vector<double>& vxOut,
	                       std::vector<double>& vyOut,
	                       bool& hadOneOut);
	void takeMergeRemaps(std::uint64_t& tickOut,
	                     std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairsOut,
	                     bool& hadOneOut);

   private:
	void flushIncoming(ENetEvent& event);

	ENetHost* host_ = nullptr;
	ENetPeer* serverPeer_ = nullptr;

	std::optional<std::uint64_t> pendingJoinTick_;
	std::uint64_t pendingJoinGlobalPhysicsStep_ = 0;
	std::vector<sim::AuthoritativeBody> pendingJoinBodies_;
	sim::BodyId pendingJoinOwnShip_ = 0;
	double pendingJoinTimeScale_ = 1.0;
	bool haveJoinAccept_ = false;

	std::vector<ShipNetSample> pendingShips_;
	std::vector<sim::BodyId> pendingWorldIds_;
	std::vector<double> pendingWorldPx_;
	std::vector<double> pendingWorldPy_;
	std::vector<double> pendingWorldVx_;
	std::vector<double> pendingWorldVy_;
	std::uint64_t pendingWorldTick_ = 0;
	std::uint64_t pendingWorldGlobalPhysicsStep_ = 0;
	bool haveWorld_ = false;

	std::uint64_t pendingMergeTick_ = 0;
	std::vector<std::pair<sim::BodyId, sim::BodyId>> pendingMerges_;
	bool haveMerge_ = false;
};

}  // namespace net
