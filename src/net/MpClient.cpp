#include "net/MpClient.hpp"

namespace net {

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
	host_ = enet_host_create(nullptr, 1, 3, 0, 0);
	if (host_ == nullptr) {
		return false;
	}
	serverPeer_ = enet_host_connect(host_, &address, 3, 0);
	return serverPeer_ != nullptr;
}

void MpClient::disconnect() {
	if (host_ != nullptr) {
		enet_host_destroy(host_);
		host_ = nullptr;
		serverPeer_ = nullptr;
	}
	haveJoinAccept_ = false;
	haveWorld_ = false;
	haveMerge_ = false;
	pendingJoinTimeScale_ = 1.0;
	pendingJoinBodies_.clear();
	pendingShips_.clear();
	pendingWorldIds_.clear();
	pendingWorldPx_.clear();
	pendingWorldPy_.clear();
	pendingWorldVx_.clear();
	pendingWorldVy_.clear();
	pendingMerges_.clear();
}

void MpClient::flushIncoming(ENetEvent& event) {
	if (event.packet->dataLength < 6) {
		enet_packet_destroy(event.packet);
		return;
	}
	const std::uint8_t* d = event.packet->data;
	const std::size_t len = event.packet->dataLength;
	if (d[4] != kProtocolVersion) {
		enet_packet_destroy(event.packet);
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
			                  s.vy, s.facingRadians, s.thrustForward, s.thrustReverse)) {
				pendingShips_.push_back(s);
			}
		} break;
		case MsgType::WorldDynamicSnapshot: {
			std::uint64_t tick = 0;
			std::uint64_t worldG = 0;
			std::vector<sim::BodyId> ids;
			std::vector<double> px;
			std::vector<double> py;
			std::vector<double> vx;
			std::vector<double> vy;
			if (readWorldDynamicSnapshot(d, len, tick, worldG, ids, px, py, vx, vy)) {
				pendingWorldTick_ = tick;
				pendingWorldGlobalPhysicsStep_ = worldG;
				pendingWorldIds_ = std::move(ids);
				pendingWorldPx_ = std::move(px);
				pendingWorldPy_ = std::move(py);
				pendingWorldVx_ = std::move(vx);
				pendingWorldVy_ = std::move(vy);
				haveWorld_ = true;
			}
		} break;
		case MsgType::MergeRemapBatch: {
			std::uint64_t tick = 0;
			std::vector<std::pair<sim::BodyId, sim::BodyId>> pairs;
			if (readMergeRemapBatch(d, len, tick, pairs)) {
				pendingMergeTick_ = tick;
				pendingMerges_ = std::move(pairs);
				haveMerge_ = true;
			}
		} break;
		default:
			break;
	}
	enet_packet_destroy(event.packet);
}

void MpClient::service(const int timeoutMs) {
	if (host_ == nullptr) {
		return;
	}
	ENetEvent event{};
	while (enet_host_service(host_, &event, timeoutMs) > 0) {
		if (event.type == ENET_EVENT_TYPE_RECEIVE) {
			flushIncoming(event);
		} else if (event.type == ENET_EVENT_TYPE_DISCONNECT) {
			serverPeer_ = nullptr;
		}
	}
}

void MpClient::sendJoinRequest() {
	if (host_ == nullptr || serverPeer_ == nullptr) {
		return;
	}
	std::vector<std::uint8_t> payload;
	writeJoinRequest(payload);
	ENetPacket* packet =
	    enet_packet_create(payload.data(), payload.size(), ENET_PACKET_FLAG_RELIABLE);
	enet_peer_send(serverPeer_, 1, packet);
	enet_host_flush(host_);
}

void MpClient::sendInput(const ClientInputPayload& payload) {
	if (host_ == nullptr || serverPeer_ == nullptr) {
		return;
	}
	std::vector<std::uint8_t> payloadBytes;
	writeClientInput(payload, payloadBytes);
	ENetPacket* packet = enet_packet_create(payloadBytes.data(), payloadBytes.size(), 0);
	enet_peer_send(serverPeer_, 0, packet);
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

void MpClient::takeWorldSnapshot(std::uint64_t& tickOut,
                                 std::uint64_t& globalPhysicsStepOut,
                                 std::vector<sim::BodyId>& idsOut,
                                 std::vector<double>& pxOut,
                                 std::vector<double>& pyOut,
                                 std::vector<double>& vxOut,
                                 std::vector<double>& vyOut,
                                 bool& hadOneOut) {
	if (!haveWorld_) {
		hadOneOut = false;
		tickOut = 0;
		globalPhysicsStepOut = 0;
		idsOut.clear();
		pxOut.clear();
		pyOut.clear();
		vxOut.clear();
		vyOut.clear();
		return;
	}
	hadOneOut = true;
	tickOut = pendingWorldTick_;
	globalPhysicsStepOut = pendingWorldGlobalPhysicsStep_;
	idsOut = std::move(pendingWorldIds_);
	pxOut = std::move(pendingWorldPx_);
	pyOut = std::move(pendingWorldPy_);
	vxOut = std::move(pendingWorldVx_);
	vyOut = std::move(pendingWorldVy_);
	haveWorld_ = false;
}

void MpClient::takeMergeRemaps(std::uint64_t& tickOut,
                               std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairsOut,
                               bool& hadOneOut) {
	if (!haveMerge_) {
		hadOneOut = false;
		tickOut = 0;
		pairsOut.clear();
		return;
	}
	hadOneOut = true;
	tickOut = pendingMergeTick_;
	pairsOut = std::move(pendingMerges_);
	haveMerge_ = false;
}

}  // namespace net
