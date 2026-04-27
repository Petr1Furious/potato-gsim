#include "net/Protocol.hpp"
#include "net/MpConstants.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <string>
#include <string_view>

namespace net {

namespace {

void appendU8(std::vector<std::uint8_t>& out, std::uint8_t v) {
	out.push_back(v);
}

void appendU32(std::vector<std::uint8_t>& out, std::uint32_t v) {
	for (int i = 0; i < 4; ++i) {
		out.push_back(static_cast<std::uint8_t>((v >> (8 * i)) & 0xFFu));
	}
}

void appendU64(std::vector<std::uint8_t>& out, std::uint64_t v) {
	for (int i = 0; i < 8; ++i) {
		out.push_back(static_cast<std::uint8_t>((v >> (8 * i)) & 0xFFu));
	}
}

void appendF32(std::vector<std::uint8_t>& out, float f) {
	std::uint32_t u = 0;
	std::memcpy(&u, &f, sizeof(u));
	appendU32(out, u);
}

void appendF64(std::vector<std::uint8_t>& out, double f) {
	std::uint64_t u = 0;
	std::memcpy(&u, &f, sizeof(u));
	appendU64(out, u);
}

bool readU32(const std::uint8_t*& p, const std::uint8_t* end, std::uint32_t& out) {
	if (end - p < 4) {
		return false;
	}
	out = static_cast<std::uint32_t>(p[0]) | (static_cast<std::uint32_t>(p[1]) << 8u) |
	      (static_cast<std::uint32_t>(p[2]) << 16u) | (static_cast<std::uint32_t>(p[3]) << 24u);
	p += 4;
	return true;
}

bool readU64(const std::uint8_t*& p, const std::uint8_t* end, std::uint64_t& out) {
	if (end - p < 8) {
		return false;
	}
	out = 0;
	for (int i = 0; i < 8; ++i) {
		out |= static_cast<std::uint64_t>(p[i]) << (8 * i);
	}
	p += 8;
	return true;
}

bool readF32(const std::uint8_t*& p, const std::uint8_t* end, float& out) {
	std::uint32_t u = 0;
	if (!readU32(p, end, u)) {
		return false;
	}
	std::memcpy(&out, &u, sizeof(out));
	return true;
}

bool readF64(const std::uint8_t*& p, const std::uint8_t* end, double& out) {
	std::uint64_t u = 0;
	if (!readU64(p, end, u)) {
		return false;
	}
	std::memcpy(&out, &u, sizeof(out));
	return true;
}

bool readHeader(const std::uint8_t*& p,
                const std::uint8_t* end,
                MsgType expected,
                std::uint8_t& versionOut) {
	if (end - p < 6) {
		return false;
	}
	std::uint32_t magic =
	    static_cast<std::uint32_t>(p[0]) | (static_cast<std::uint32_t>(p[1]) << 8u) |
	    (static_cast<std::uint32_t>(p[2]) << 16u) | (static_cast<std::uint32_t>(p[3]) << 24u);
	p += 4;
	versionOut = *p++;
	const std::uint8_t t = *p++;
	if (magic != kMagic || versionOut != kProtocolVersion || static_cast<MsgType>(t) != expected) {
		return false;
	}
	return true;
}

void writeHeader(std::vector<std::uint8_t>& out, MsgType type) {
	appendU32(out, kMagic);
	appendU8(out, kProtocolVersion);
	appendU8(out, static_cast<std::uint8_t>(type));
}

void appendAuthoritativeBodyRow(std::vector<std::uint8_t>& out, const sim::AuthoritativeBody& b) {
	appendU64(out, b.id);
	appendF64(out, b.x);
	appendF64(out, b.y);
	appendF64(out, b.vx);
	appendF64(out, b.vy);
	appendF64(out, b.mass);
	appendF64(out, b.radius);
	const std::uint32_t nl =
	    static_cast<std::uint32_t>(std::min<std::size_t>(b.name.size(), 65000));
	appendU32(out, nl);
	for (std::uint32_t i = 0; i < nl; ++i) {
		out.push_back(static_cast<std::uint8_t>(b.name[i]));
	}
}

bool readAuthoritativeBodyRow(const std::uint8_t*& p,
                              const std::uint8_t* end,
                              sim::AuthoritativeBody& b) {
	if (!readU64(p, end, b.id)) {
		return false;
	}
	if (!readF64(p, end, b.x) || !readF64(p, end, b.y) || !readF64(p, end, b.vx) ||
	    !readF64(p, end, b.vy) || !readF64(p, end, b.mass) || !readF64(p, end, b.radius)) {
		return false;
	}
	std::uint32_t nameLen = 0;
	if (!readU32(p, end, nameLen) || nameLen > 65000u ||
	    static_cast<std::size_t>(end - p) < nameLen) {
		return false;
	}
	b.name.assign(reinterpret_cast<const char*>(p), nameLen);
	p += nameLen;
	return true;
}

}  // namespace

bool writeJoinRequest(const std::string_view nameUtf8, std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::JoinRequest);
	std::size_t n = nameUtf8.size();
	while (n > 0 && (nameUtf8[n - 1] == ' ' || nameUtf8[n - 1] == '\t')) {
		--n;
	}
	std::size_t start = 0;
	while (start < n && (nameUtf8[start] == ' ' || nameUtf8[start] == '\t')) {
		++start;
	}
	const std::size_t trimmed = (start < n) ? (n - start) : 0;
	const std::size_t enc = std::min(trimmed, static_cast<std::size_t>(kJoinRequestNameMaxBytes));
	appendU32(out, static_cast<std::uint32_t>(enc));
	for (std::size_t i = 0; i < enc; ++i) {
		out.push_back(static_cast<std::uint8_t>(nameUtf8[start + i]));
	}
	return true;
}

bool readJoinRequest(const std::uint8_t* data, const std::size_t len, std::string& nameOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	nameOut.clear();
	if (!readHeader(p, end, MsgType::JoinRequest, ver)) {
		return false;
	}
	if (p == end) {
		return true;
	}
	std::uint32_t nameLen = 0;
	if (!readU32(p, end, nameLen) || nameLen > kJoinRequestNameMaxBytes ||
	    static_cast<std::size_t>(end - p) < nameLen) {
		return false;
	}
	nameOut.assign(reinterpret_cast<const char*>(p), nameLen);
	p += nameLen;
	return p == end;
}

bool writeJoinReject(const JoinRejectReason reason,
                     const std::string_view detailUtf8,
                     std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::JoinReject);
	appendU8(out, static_cast<std::uint8_t>(reason));
	const std::uint32_t dl =
	    static_cast<std::uint32_t>(std::min<std::size_t>(detailUtf8.size(), 512));
	appendU32(out, dl);
	for (std::uint32_t i = 0; i < dl; ++i) {
		out.push_back(static_cast<std::uint8_t>(detailUtf8[i]));
	}
	return true;
}

bool readJoinReject(const std::uint8_t* data,
                    const std::size_t len,
                    JoinRejectReason& reasonOut,
                    std::string& detailOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	detailOut.clear();
	if (!readHeader(p, end, MsgType::JoinReject, ver)) {
		return false;
	}
	if (end - p < 1) {
		return false;
	}
	reasonOut = static_cast<JoinRejectReason>(*p++);
	std::uint32_t dl = 0;
	if (!readU32(p, end, dl) || dl > 512u || static_cast<std::size_t>(end - p) < dl) {
		return false;
	}
	detailOut.assign(reinterpret_cast<const char*>(p), dl);
	p += dl;
	return p == end;
}

bool writeJoinAccept(const std::uint64_t serverTick,
                     const std::uint64_t joinGlobalPhysicsStep,
                     const std::vector<sim::AuthoritativeBody>& bodies,
                     const sim::BodyId ownShipBodyId,
                     const double serverTimeScale,
                     const double realSecondsPerPhysicsStep,
                     const double shipThrustAccel,
                     std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::JoinAccept);
	appendU64(out, serverTick);
	appendU32(out, static_cast<std::uint32_t>(bodies.size()));
	for (const sim::AuthoritativeBody& b : bodies) {
		appendAuthoritativeBodyRow(out, b);
	}
	appendU64(out, ownShipBodyId);
	appendF64(out, serverTimeScale);
	appendU64(out, joinGlobalPhysicsStep);
	appendF64(out, realSecondsPerPhysicsStep);
	appendF64(out, shipThrustAccel);
	return true;
}

bool readJoinAccept(const std::uint8_t* data,
                    const std::size_t len,
                    std::uint64_t& serverTickOut,
                    std::uint64_t& joinGlobalPhysicsStepOut,
                    std::vector<sim::AuthoritativeBody>& bodiesOut,
                    sim::BodyId& ownShipBodyIdOut,
                    double& serverTimeScaleOut,
                    double& realSecondsPerPhysicsStepOut,
                    double& shipThrustAccelOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	ownShipBodyIdOut = 0;
	serverTimeScaleOut = 1.0;
	joinGlobalPhysicsStepOut = 0;
	realSecondsPerPhysicsStepOut = kDefaultRealSecondsPerPhysicsStep;
	shipThrustAccelOut = kDefaultShipThrustAccel;
	if (!readHeader(p, end, MsgType::JoinAccept, ver)) {
		return false;
	}
	if (!readU64(p, end, serverTickOut)) {
		return false;
	}
	std::uint32_t n = 0;
	if (!readU32(p, end, n) || n > 2'000'000u) {
		return false;
	}
	bodiesOut.clear();
	bodiesOut.reserve(n);
	for (std::uint32_t i = 0; i < n; ++i) {
		sim::AuthoritativeBody b{};
		if (!readAuthoritativeBodyRow(p, end, b)) {
			return false;
		}
		bodiesOut.push_back(std::move(b));
	}
	if (static_cast<std::size_t>(end - p) >= 8) {
		if (!readU64(p, end, ownShipBodyIdOut)) {
			return false;
		}
	}
	if (static_cast<std::size_t>(end - p) >= 8) {
		double ts = 0.0;
		if (!readF64(p, end, ts)) {
			return false;
		}
		if (std::isfinite(ts) && ts > 0.0) {
			serverTimeScaleOut = ts;
		}
	}
	if (static_cast<std::size_t>(end - p) >= 8) {
		if (!readU64(p, end, joinGlobalPhysicsStepOut)) {
			return false;
		}
	}
	if (static_cast<std::size_t>(end - p) >= 16) {
		double rs = 0.0;
		double ta = 0.0;
		if (!readF64(p, end, rs) || !readF64(p, end, ta)) {
			return false;
		}
		if (std::isfinite(rs) && rs > 0.0) {
			realSecondsPerPhysicsStepOut = rs;
		}
		if (std::isfinite(ta) && ta > 0.0) {
			shipThrustAccelOut = ta;
		}
	}
	return p == end;
}

bool writeClientInput(const ClientInputPayload& in, std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::ClientInput);
	appendU32(out, in.seq);
	appendU8(out, in.thrustForward);
	appendF32(out, in.facingRadians);
	appendU8(out, in.thrustPercent);
	appendU8(out, in.firePrimary);
	appendF32(out, in.shellAimRadians);
	appendF32(out, in.shellExtraSpeed);
	return true;
}

bool readClientInput(const std::uint8_t* data, const std::size_t len, ClientInputPayload& out) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	if (!readHeader(p, end, MsgType::ClientInput, ver)) {
		return false;
	}
	if (!readU32(p, end, out.seq)) {
		return false;
	}
	if (end - p < 1) {
		return false;
	}
	out.thrustForward = *p++;
	if (!readF32(p, end, out.facingRadians)) {
		return false;
	}
	if (end - p < 1) {
		return false;
	}
	out.thrustPercent = *p++;
	out.firePrimary = 0;
	out.shellAimRadians = out.facingRadians;
	out.shellExtraSpeed = 0.f;
	if (static_cast<std::size_t>(end - p) >= 1) {
		out.firePrimary = *p++;
	}
	if (static_cast<std::size_t>(end - p) >= 4) {
		if (!readF32(p, end, out.shellAimRadians)) {
			return false;
		}
	}
	if (static_cast<std::size_t>(end - p) >= 4) {
		if (!readF32(p, end, out.shellExtraSpeed)) {
			return false;
		}
	}
	return p == end;
}

bool writeShipStateBatch(const std::uint64_t serverTick,
                         const std::uint64_t globalPhysicsStep,
                         const std::vector<ShipStateWire>& ships,
                         std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::ShipState);
	appendU64(out, serverTick);
	appendU64(out, globalPhysicsStep);
	appendU32(out, static_cast<std::uint32_t>(ships.size()));
	for (const ShipStateWire& s : ships) {
		appendU64(out, s.bodyId);
		appendF64(out, s.px);
		appendF64(out, s.py);
		appendF64(out, s.vx);
		appendF64(out, s.vy);
		appendF32(out, s.facing);
		appendU8(out, s.thrustForward);
		appendU8(out, s.thrustPercent);
		appendF32(out, s.deltaVCurrentMps);
		appendF32(out, s.deltaVMaxMps);
		appendU64(out, s.shellReadyGlobalPhysicsStep);
	}
	return true;
}

bool readShipStateBatch(const std::uint8_t* data,
                        const std::size_t len,
                        std::uint64_t& tickOut,
                        std::uint64_t& globalPhysicsStepOut,
                        std::vector<ShipStateWire>& shipsOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	globalPhysicsStepOut = 0;
	shipsOut.clear();
	if (!readHeader(p, end, MsgType::ShipState, ver)) {
		return false;
	}
	if (!readU64(p, end, tickOut) || !readU64(p, end, globalPhysicsStepOut)) {
		return false;
	}
	std::uint32_t n = 0;
	if (!readU32(p, end, n) || n > 1'000'000u) {
		return false;
	}
	shipsOut.reserve(n);
	for (std::uint32_t i = 0; i < n; ++i) {
		ShipStateWire s{};
		s.thrustPercent = 100;
		if (!readU64(p, end, s.bodyId) || !readF64(p, end, s.px) || !readF64(p, end, s.py) ||
		    !readF64(p, end, s.vx) || !readF64(p, end, s.vy) || !readF32(p, end, s.facing)) {
			return false;
		}
		if (end - p < 2) {
			return false;
		}
		s.thrustForward = p[0];
		s.thrustPercent = p[1];
		p += 2;
		if (static_cast<std::size_t>(end - p) >= 8) {
			if (!readF32(p, end, s.deltaVCurrentMps) || !readF32(p, end, s.deltaVMaxMps)) {
				return false;
			}
		}
		if (static_cast<std::size_t>(end - p) >= 8) {
			if (!readU64(p, end, s.shellReadyGlobalPhysicsStep)) {
				return false;
			}
		}
		shipsOut.push_back(s);
	}
	return p == end;
}

bool writeWorldDynamicSnapshot(const std::uint64_t serverTick,
                               const std::uint64_t globalPhysicsStep,
                               const std::vector<sim::AuthoritativeBody>& bodies,
                               std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::WorldDynamicSnapshot);
	appendU64(out, serverTick);
	appendU64(out, globalPhysicsStep);
	appendU32(out, static_cast<std::uint32_t>(bodies.size()));
	for (const sim::AuthoritativeBody& b : bodies) {
		appendAuthoritativeBodyRow(out, b);
	}
	return true;
}

bool readWorldDynamicSnapshot(const std::uint8_t* data,
                              const std::size_t len,
                              std::uint64_t& tickOut,
                              std::uint64_t& globalPhysicsStepOut,
                              std::vector<sim::AuthoritativeBody>& bodiesOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	globalPhysicsStepOut = 0;
	if (!readHeader(p, end, MsgType::WorldDynamicSnapshot, ver)) {
		return false;
	}
	if (!readU64(p, end, tickOut) || !readU64(p, end, globalPhysicsStepOut)) {
		return false;
	}
	std::uint32_t n = 0;
	if (!readU32(p, end, n) || n > 2'000'000u) {
		return false;
	}
	bodiesOut.clear();
	bodiesOut.reserve(n);
	for (std::uint32_t i = 0; i < n; ++i) {
		sim::AuthoritativeBody b{};
		if (!readAuthoritativeBodyRow(p, end, b)) {
			return false;
		}
		bodiesOut.push_back(std::move(b));
	}
	return p == end;
}

bool writeMergeRemapBatch(const std::uint64_t serverTick,
                          const std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairs,
                          std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::MergeRemapBatch);
	appendU64(out, serverTick);
	appendU32(out, static_cast<std::uint32_t>(pairs.size()));
	for (const auto& pr : pairs) {
		appendU64(out, pr.first);
		appendU64(out, pr.second);
	}
	return true;
}

bool readMergeRemapBatch(const std::uint8_t* data,
                         const std::size_t len,
                         std::uint64_t& tickOut,
                         std::vector<std::pair<sim::BodyId, sim::BodyId>>& pairsOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	if (!readHeader(p, end, MsgType::MergeRemapBatch, ver)) {
		return false;
	}
	if (!readU64(p, end, tickOut)) {
		return false;
	}
	std::uint32_t n = 0;
	if (!readU32(p, end, n) || n > 1'000'000u) {
		return false;
	}
	pairsOut.resize(n);
	for (std::uint32_t i = 0; i < n; ++i) {
		sim::BodyId a = 0;
		sim::BodyId b = 0;
		if (!readU64(p, end, a) || !readU64(p, end, b)) {
			return false;
		}
		pairsOut[i] = {a, b};
	}
	return p == end;
}

bool writeAuthoritativeBodyUpsert(const std::uint64_t serverTick,
                                  const std::uint64_t globalPhysicsStep,
                                  const std::vector<sim::AuthoritativeBody>& bodies,
                                  std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::AuthoritativeBodyUpsert);
	appendU64(out, serverTick);
	appendU64(out, globalPhysicsStep);
	appendU32(out, static_cast<std::uint32_t>(bodies.size()));
	for (const sim::AuthoritativeBody& b : bodies) {
		appendAuthoritativeBodyRow(out, b);
	}
	return true;
}

bool readAuthoritativeBodyUpsert(const std::uint8_t* data,
                                 const std::size_t len,
                                 std::uint64_t& serverTickOut,
                                 std::uint64_t& globalPhysicsStepOut,
                                 std::vector<sim::AuthoritativeBody>& bodiesOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	serverTickOut = 0;
	globalPhysicsStepOut = 0;
	if (!readHeader(p, end, MsgType::AuthoritativeBodyUpsert, ver)) {
		return false;
	}
	if (!readU64(p, end, serverTickOut) || !readU64(p, end, globalPhysicsStepOut)) {
		return false;
	}
	std::uint32_t n = 0;
	if (!readU32(p, end, n) || n > 2'000'000u) {
		return false;
	}
	bodiesOut.clear();
	bodiesOut.reserve(n);
	for (std::uint32_t i = 0; i < n; ++i) {
		sim::AuthoritativeBody b{};
		if (!readAuthoritativeBodyRow(p, end, b)) {
			return false;
		}
		bodiesOut.push_back(std::move(b));
	}
	return p == end;
}

bool writeBodyDeleteBatch(const std::uint64_t serverTick,
                          const std::uint64_t globalPhysicsStep,
                          const std::vector<sim::BodyId>& ids,
                          std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::BodyDeleteBatch);
	appendU64(out, serverTick);
	appendU64(out, globalPhysicsStep);
	appendU32(out, static_cast<std::uint32_t>(ids.size()));
	for (const sim::BodyId id : ids) {
		appendU64(out, id);
	}
	return true;
}

bool readBodyDeleteBatch(const std::uint8_t* data,
                         const std::size_t len,
                         std::uint64_t& serverTickOut,
                         std::uint64_t& globalPhysicsStepOut,
                         std::vector<sim::BodyId>& idsOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	serverTickOut = 0;
	globalPhysicsStepOut = 0;
	if (!readHeader(p, end, MsgType::BodyDeleteBatch, ver)) {
		return false;
	}
	if (!readU64(p, end, serverTickOut) || !readU64(p, end, globalPhysicsStepOut)) {
		return false;
	}
	std::uint32_t n = 0;
	if (!readU32(p, end, n) || n > 1'000'000u) {
		return false;
	}
	idsOut.clear();
	idsOut.reserve(n);
	for (std::uint32_t i = 0; i < n; ++i) {
		sim::BodyId id = 0;
		if (!readU64(p, end, id)) {
			return false;
		}
		if (id != 0) {
			idsOut.push_back(id);
		}
	}
	return p == end;
}

bool writeRespawnCountdown(const std::uint64_t serverTick,
                           const std::uint64_t respawnAtServerTick,
                           const double wallSecondsRemaining,
                           std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::RespawnCountdown);
	appendU64(out, serverTick);
	appendU64(out, respawnAtServerTick);
	appendF64(out, wallSecondsRemaining);
	return true;
}

bool readRespawnCountdown(const std::uint8_t* data,
                          const std::size_t len,
                          std::uint64_t& serverTickOut,
                          std::uint64_t& respawnAtServerTickOut,
                          double& wallSecondsRemainingOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	serverTickOut = 0;
	respawnAtServerTickOut = 0;
	wallSecondsRemainingOut = 0.0;
	if (!readHeader(p, end, MsgType::RespawnCountdown, ver)) {
		return false;
	}
	if (!readU64(p, end, serverTickOut) || !readU64(p, end, respawnAtServerTickOut)) {
		return false;
	}
	if (!readF64(p, end, wallSecondsRemainingOut)) {
		return false;
	}
	return p == end;
}

}  // namespace net
