#pragma once

#include "net/Protocol.hpp"
#include "sim/BodyId.hpp"
#include "sim/CommandQueue.hpp"

#include <enet/enet.h>

#include <atomic>
#include <cstdint>
#include <deque>
#include <mutex>
#include <optional>
#include <stop_token>
#include <string>
#include <thread>
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
		std::uint8_t thrustPercent = 100;
		float deltaVCurrentMps = 0.f;
		float deltaVMaxMps = 0.f;
		/// First `globalPhysicsStep` at which shell may fire again (`<=` sample step means ready).
		std::uint64_t shellReadyGlobalPhysicsStep = 0;
	};

	MpClient() = default;
	MpClient(const MpClient&) = delete;
	MpClient& operator=(const MpClient&) = delete;
	MpClient(MpClient&&) = delete;
	MpClient& operator=(MpClient&&) = delete;
	~MpClient();

	[[nodiscard]] bool connect(const std::string& host, std::uint16_t port);
	void disconnect();

	/// Main thread: apply packets received on the background net thread (non-blocking).
	void service(int timeoutMs);

#if defined(POTATO_GSIM_MP_WORLD_SYNC_TESTS)
	/// Unit tests: enqueue raw inbound bytes as if received from the network thread.
	void testingEnqueueInboundPacket(std::vector<std::uint8_t> packet);
#endif

	void sendJoinRequest();
	void sendInput(const ClientInputPayload& payload);

	[[nodiscard]] bool isConnected() const;
	/// True once the outgoing connection handshake has completed (safe to send join).
	[[nodiscard]] bool isPeerConnected() const;

	void takeJoinAccept(std::uint64_t& tickOut,
	                    std::uint64_t& joinGlobalPhysicsStepOut,
	                    std::vector<sim::AuthoritativeBody>& bodiesOut,
	                    sim::BodyId& ownShipBodyIdOut,
	                    double& serverTimeScaleOut);
	void takeShipSamples(std::vector<ShipNetSample>& out);
	/// Pops one queued world snapshot (FIFO). Returns false when empty.
	[[nodiscard]] bool takeNextWorldSnapshot(std::uint64_t& tickOut,
	                                         std::uint64_t& globalPhysicsStepOut,
	                                         std::vector<sim::AuthoritativeBody>& bodiesOut);
	/// Pops one queued merge batch (FIFO). Returns false when empty.
	[[nodiscard]] bool takeNextMergeRemaps(
	    std::uint64_t& tickOut,
	    std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairsOut);
	/// Pops one explicit body-delete batch (FIFO). Returns false when empty.
	[[nodiscard]] bool takeNextBodyDeleteBatch(std::uint64_t& tickOut,
	                                           std::uint64_t& globalPhysicsStepOut,
	                                           std::vector<sim::BodyId>& idsOut);
	void takeAuthoritativeUpserts(std::vector<sim::AuthoritativeBody>& bodiesOut, bool& hadOneOut);

   private:
	enum class OutboundKind : std::uint8_t { JoinReliable, InputUnreliable };

	void netThreadMain(std::stop_token st);
	void enqueueReceivedPacket(std::vector<std::uint8_t> bytes);
	void processPacket(const std::uint8_t* data, std::size_t len);

	struct PendingWorldSnapshot {
		std::uint64_t serverTick = 0;
		std::uint64_t globalPhysicsStep = 0;
		std::vector<sim::AuthoritativeBody> bodies;
	};
	struct PendingMergeBatch {
		std::uint64_t serverTick = 0;
		std::vector<std::pair<sim::BodyId, sim::BodyId>> pairs;
	};
	struct PendingBodyDeleteBatch {
		std::uint64_t serverTick = 0;
		std::uint64_t globalPhysicsStep = 0;
		std::vector<sim::BodyId> ids;
	};

	ENetHost* host_ = nullptr;
	ENetPeer* serverPeer_ = nullptr;
	mutable std::mutex enetMutex_;
	std::optional<std::jthread> netThread_;

	/// Outbound payloads built on the main thread; only the net thread calls `enet_peer_send`.
	std::mutex sendMutex_;
	std::deque<std::pair<OutboundKind, std::vector<std::uint8_t>>> outboundPackets_;

	std::atomic<bool> hasServerPeer_{false};
	std::atomic<bool> peerFullyConnected_{false};

	std::mutex inboundMutex_;
	std::deque<std::vector<std::uint8_t>> inboundPackets_;

	std::optional<std::uint64_t> pendingJoinTick_;
	std::uint64_t pendingJoinGlobalPhysicsStep_ = 0;
	std::vector<sim::AuthoritativeBody> pendingJoinBodies_;
	sim::BodyId pendingJoinOwnShip_ = 0;
	double pendingJoinTimeScale_ = 1.0;
	bool haveJoinAccept_ = false;

	std::vector<ShipNetSample> pendingShips_;
	std::deque<PendingWorldSnapshot> pendingWorldSnapshots_;
	std::deque<PendingMergeBatch> pendingMergeBatches_;
	std::deque<PendingBodyDeleteBatch> pendingBodyDeleteBatches_;

	std::vector<sim::AuthoritativeBody> pendingAuthoritativeUpserts_;
	bool haveAuthoritativeUpserts_ = false;
};

}  // namespace net
