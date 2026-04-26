#include "net/MpClient.hpp"

#include <chrono>
#include <cstring>
#include <iterator>
#include <thread>
#include <vector>

namespace net {

namespace {

// Large enough for bursts; oldest packets are dropped if the main thread stalls.
constexpr std::size_t kInboundPacketQueueMax = 8192;
constexpr std::size_t kOutboundQueueMax = 8192;

}  // namespace

MpClient::~MpClient() {
	disconnect();
}

bool MpClient::connect(const std::string& host, const std::uint16_t port) {
	disconnect();
	ENetAddress address{};
	if (enet_address_set_host(&address, host.c_str()) != 0) {
		return false;
	}
	address.port = port;
	{
		std::lock_guard<std::mutex> lock(enetMutex_);
		host_ = enet_host_create(nullptr, 1, 3, 0, 0);
		if (host_ == nullptr) {
			return false;
		}
		serverPeer_ = enet_host_connect(host_, &address, 3, 0);
		if (serverPeer_ == nullptr) {
			enet_host_destroy(host_);
			host_ = nullptr;
			return false;
		}
		hasServerPeer_.store(true, std::memory_order_release);
		peerFullyConnected_.store(false, std::memory_order_release);
	}
	netThread_.emplace([this](std::stop_token st) { netThreadMain(st); });
	return true;
}

void MpClient::disconnect() {
	if (netThread_.has_value()) {
		netThread_->request_stop();
		netThread_.reset();
	}
	hasServerPeer_.store(false, std::memory_order_release);
	peerFullyConnected_.store(false, std::memory_order_release);
	{
		std::lock_guard<std::mutex> q(inboundMutex_);
		inboundPackets_.clear();
	}
	{
		std::lock_guard<std::mutex> s(sendMutex_);
		outboundPackets_.clear();
	}
	std::lock_guard<std::mutex> lock(enetMutex_);
	if (host_ != nullptr) {
		enet_host_destroy(host_);
		host_ = nullptr;
	}
	serverPeer_ = nullptr;
	haveJoinAccept_ = false;
	pendingJoinTimeScale_ = 1.0;
	pendingJoinBodies_.clear();
	pendingShips_.clear();
	pendingWorldSnapshots_.clear();
	pendingMergeBatches_.clear();
	pendingBodyDeleteBatches_.clear();
	pendingAuthoritativeUpserts_.clear();
	haveAuthoritativeUpserts_ = false;
}

void MpClient::processPacket(const std::uint8_t* d, const std::size_t len) {
	if (len < 6) {
		return;
	}
	if (d[4] != kProtocolVersion) {
		return;
	}
	const auto type = static_cast<MsgType>(d[5]);
	switch (type) {
		case MsgType::JoinAccept: {
			std::uint64_t tick = 0;
			std::uint64_t joinG = 0;
			sim::BodyId ownShip = 0;
			double joinTs = 1.0;
			std::vector<sim::AuthoritativeBody> bodies;
			if (readJoinAccept(d, len, tick, joinG, bodies, ownShip, joinTs)) {
				pendingJoinTick_ = tick;
				pendingJoinGlobalPhysicsStep_ = joinG;
				pendingJoinBodies_ = std::move(bodies);
				pendingJoinOwnShip_ = ownShip;
				pendingJoinTimeScale_ = joinTs;
				haveJoinAccept_ = true;
			}
		} break;
		case MsgType::ShipState: {
			ShipNetSample s{};
			if (readShipState(d, len, s.serverTick, s.globalPhysicsStep, s.bodyId, s.px, s.py, s.vx,
			                  s.vy, s.facingRadians, s.thrustForward, s.thrustPercent,
			                  s.deltaVCurrentMps, s.deltaVMaxMps, s.shellReadyGlobalPhysicsStep)) {
				pendingShips_.push_back(s);
			}
		} break;
		case MsgType::WorldDynamicSnapshot: {
			std::uint64_t tick = 0;
			std::uint64_t worldG = 0;
			std::vector<sim::AuthoritativeBody> bodies;
			if (readWorldDynamicSnapshot(d, len, tick, worldG, bodies)) {
				PendingWorldSnapshot snap;
				snap.serverTick = tick;
				snap.globalPhysicsStep = worldG;
				snap.bodies = std::move(bodies);
				pendingWorldSnapshots_.push_back(std::move(snap));
			}
		} break;
		case MsgType::MergeRemapBatch: {
			std::uint64_t tick = 0;
			std::vector<std::pair<sim::BodyId, sim::BodyId>> pairs;
			if (readMergeRemapBatch(d, len, tick, pairs)) {
				PendingMergeBatch batch;
				batch.serverTick = tick;
				batch.pairs = std::move(pairs);
				pendingMergeBatches_.push_back(std::move(batch));
			}
		} break;
		case MsgType::AuthoritativeBodyUpsert: {
			std::uint64_t tick = 0;
			std::uint64_t step = 0;
			std::vector<sim::AuthoritativeBody> bodies;
			if (readAuthoritativeBodyUpsert(d, len, tick, step, bodies)) {
				(void)tick;
				(void)step;
				pendingAuthoritativeUpserts_.insert(pendingAuthoritativeUpserts_.end(),
				                                    std::make_move_iterator(bodies.begin()),
				                                    std::make_move_iterator(bodies.end()));
				haveAuthoritativeUpserts_ = true;
			}
		} break;
		case MsgType::BodyDeleteBatch: {
			std::uint64_t tick = 0;
			std::uint64_t step = 0;
			std::vector<sim::BodyId> ids;
			if (readBodyDeleteBatch(d, len, tick, step, ids)) {
				PendingBodyDeleteBatch batch;
				batch.serverTick = tick;
				batch.globalPhysicsStep = step;
				batch.ids = std::move(ids);
				pendingBodyDeleteBatches_.push_back(std::move(batch));
			}
		} break;
		default:
			break;
	}
}

void MpClient::enqueueReceivedPacket(std::vector<std::uint8_t> bytes) {
	std::lock_guard<std::mutex> lock(inboundMutex_);
	while (inboundPackets_.size() >= kInboundPacketQueueMax) {
		inboundPackets_.pop_front();
	}
	inboundPackets_.push_back(std::move(bytes));
}

void MpClient::netThreadMain(const std::stop_token st) {
	while (!st.stop_requested()) {
		std::deque<std::pair<OutboundKind, std::vector<std::uint8_t>>> outBatch;
		{
			std::lock_guard<std::mutex> s(sendMutex_);
			outBatch.swap(outboundPackets_);
		}

		bool hadNetActivity = false;
		std::vector<std::vector<std::uint8_t>> recvBatch;
		{
			std::lock_guard<std::mutex> lock(enetMutex_);
			if (host_ == nullptr) {
				break;
			}
			bool sentAny = false;
			for (std::pair<OutboundKind, std::vector<std::uint8_t>>& item : outBatch) {
				if (serverPeer_ == nullptr) {
					break;
				}
				const OutboundKind kind = item.first;
				std::vector<std::uint8_t>& bytes = item.second;
				if (bytes.empty()) {
					continue;
				}
				const std::uint32_t flags =
				    (kind == OutboundKind::JoinReliable) ? ENET_PACKET_FLAG_RELIABLE : 0;
				const std::uint8_t channel = (kind == OutboundKind::JoinReliable) ? 1 : 0;
				ENetPacket* packet = enet_packet_create(bytes.data(), bytes.size(), flags);
				if (packet == nullptr) {
					continue;
				}
				enet_peer_send(serverPeer_, channel, packet);
				sentAny = true;
			}
			if (sentAny) {
				enet_host_flush(host_);
				hadNetActivity = true;
			}

			ENetEvent event{};
			while (enet_host_service(host_, &event, 0) > 0) {
				hadNetActivity = true;
				if (event.type == ENET_EVENT_TYPE_RECEIVE) {
					std::vector<std::uint8_t> payload;
					if (event.packet->dataLength > 0 && event.packet->data != nullptr) {
						payload.resize(event.packet->dataLength);
						std::memcpy(payload.data(), event.packet->data, event.packet->dataLength);
					}
					enet_packet_destroy(event.packet);
					recvBatch.push_back(std::move(payload));
				} else if (event.type == ENET_EVENT_TYPE_DISCONNECT) {
					serverPeer_ = nullptr;
					hasServerPeer_.store(false, std::memory_order_release);
					peerFullyConnected_.store(false, std::memory_order_release);
				}
			}

			if (serverPeer_ != nullptr && serverPeer_->state == ENET_PEER_STATE_CONNECTED) {
				peerFullyConnected_.store(true, std::memory_order_release);
			} else {
				peerFullyConnected_.store(false, std::memory_order_release);
			}
		}

		for (std::vector<std::uint8_t>& payload : recvBatch) {
			enqueueReceivedPacket(std::move(payload));
		}

		if (!hadNetActivity && !st.stop_requested()) {
			std::this_thread::sleep_for(std::chrono::milliseconds(1));
		}
	}
}

void MpClient::service(const int /*timeoutMs*/) {
	std::deque<std::vector<std::uint8_t>> batch;
	{
		std::lock_guard<std::mutex> lock(inboundMutex_);
		batch.swap(inboundPackets_);
	}
	for (std::vector<std::uint8_t>& bytes : batch) {
		processPacket(bytes.data(), bytes.size());
	}
}

#if defined(POTATO_GSIM_MP_WORLD_SYNC_TESTS)
void MpClient::testingEnqueueInboundPacket(std::vector<std::uint8_t> packet) {
	enqueueReceivedPacket(std::move(packet));
}
#endif

bool MpClient::isConnected() const {
	return hasServerPeer_.load(std::memory_order_acquire);
}

bool MpClient::isPeerConnected() const {
	return peerFullyConnected_.load(std::memory_order_acquire);
}

void MpClient::sendJoinRequest() {
	if (!hasServerPeer_.load(std::memory_order_acquire)) {
		return;
	}
	std::vector<std::uint8_t> payload;
	writeJoinRequest(payload);
	std::lock_guard<std::mutex> lock(sendMutex_);
	while (outboundPackets_.size() >= kOutboundQueueMax) {
		outboundPackets_.pop_front();
	}
	outboundPackets_.emplace_back(OutboundKind::JoinReliable, std::move(payload));
}

void MpClient::sendInput(const ClientInputPayload& payload) {
	if (!hasServerPeer_.load(std::memory_order_acquire)) {
		return;
	}
	std::vector<std::uint8_t> payloadBytes;
	writeClientInput(payload, payloadBytes);
	std::lock_guard<std::mutex> lock(sendMutex_);
	while (outboundPackets_.size() >= kOutboundQueueMax) {
		outboundPackets_.pop_front();
	}
	outboundPackets_.emplace_back(OutboundKind::InputUnreliable, std::move(payloadBytes));
}

void MpClient::takeJoinAccept(std::uint64_t& tickOut,
                              std::uint64_t& joinGlobalPhysicsStepOut,
                              std::vector<sim::AuthoritativeBody>& bodiesOut,
                              sim::BodyId& ownShipBodyIdOut,
                              double& serverTimeScaleOut) {
	if (!haveJoinAccept_) {
		tickOut = 0;
		joinGlobalPhysicsStepOut = 0;
		bodiesOut.clear();
		ownShipBodyIdOut = 0;
		serverTimeScaleOut = 1.0;
		return;
	}
	tickOut = *pendingJoinTick_;
	joinGlobalPhysicsStepOut = pendingJoinGlobalPhysicsStep_;
	bodiesOut = std::move(pendingJoinBodies_);
	ownShipBodyIdOut = pendingJoinOwnShip_;
	serverTimeScaleOut = pendingJoinTimeScale_;
	haveJoinAccept_ = false;
	pendingJoinTick_.reset();
	pendingJoinGlobalPhysicsStep_ = 0;
	pendingJoinOwnShip_ = 0;
	pendingJoinTimeScale_ = 1.0;
}

void MpClient::takeShipSamples(std::vector<ShipNetSample>& out) {
	out.clear();
	out.swap(pendingShips_);
}

bool MpClient::takeNextWorldSnapshot(std::uint64_t& tickOut,
                                     std::uint64_t& globalPhysicsStepOut,
                                     std::vector<sim::AuthoritativeBody>& bodiesOut) {
	if (pendingWorldSnapshots_.empty()) {
		tickOut = 0;
		globalPhysicsStepOut = 0;
		bodiesOut.clear();
		return false;
	}
	const PendingWorldSnapshot w = std::move(pendingWorldSnapshots_.front());
	pendingWorldSnapshots_.pop_front();
	tickOut = w.serverTick;
	globalPhysicsStepOut = w.globalPhysicsStep;
	bodiesOut = std::move(w.bodies);
	return true;
}

bool MpClient::takeNextMergeRemaps(std::uint64_t& tickOut,
                                   std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairsOut) {
	if (pendingMergeBatches_.empty()) {
		tickOut = 0;
		pairsOut.clear();
		return false;
	}
	const PendingMergeBatch b = std::move(pendingMergeBatches_.front());
	pendingMergeBatches_.pop_front();
	tickOut = b.serverTick;
	pairsOut = std::move(b.pairs);
	return true;
}

bool MpClient::takeNextBodyDeleteBatch(std::uint64_t& tickOut,
                                       std::uint64_t& globalPhysicsStepOut,
                                       std::vector<sim::BodyId>& idsOut) {
	if (pendingBodyDeleteBatches_.empty()) {
		tickOut = 0;
		globalPhysicsStepOut = 0;
		idsOut.clear();
		return false;
	}
	const PendingBodyDeleteBatch b = std::move(pendingBodyDeleteBatches_.front());
	pendingBodyDeleteBatches_.pop_front();
	tickOut = b.serverTick;
	globalPhysicsStepOut = b.globalPhysicsStep;
	idsOut = std::move(b.ids);
	return true;
}

void MpClient::takeAuthoritativeUpserts(std::vector<sim::AuthoritativeBody>& bodiesOut,
                                        bool& hadOneOut) {
	if (!haveAuthoritativeUpserts_) {
		hadOneOut = false;
		bodiesOut.clear();
		return;
	}
	hadOneOut = true;
	bodiesOut = std::move(pendingAuthoritativeUpserts_);
	haveAuthoritativeUpserts_ = false;
}

}  // namespace net
