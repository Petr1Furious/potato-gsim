#include "net/Protocol.hpp"

#include <cmath>
#include <cstring>
#include <string>

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

}  // namespace

bool writeJoinRequest(std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::JoinRequest);
	return true;
}

bool readJoinRequest(const std::uint8_t* data, std::size_t len) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	return readHeader(p, end, MsgType::JoinRequest, ver) && p == end;
}

bool writeJoinAccept(const std::uint64_t serverTick,
                     const std::uint64_t joinGlobalPhysicsStep,
                     const std::vector<sim::AuthoritativeBody>& bodies,
                     const sim::BodyId ownShipBodyId,
                     const double serverTimeScale,
                     std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::JoinAccept);
	appendU64(out, serverTick);
	appendU32(out, static_cast<std::uint32_t>(bodies.size()));
	for (const sim::AuthoritativeBody& b : bodies) {
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
	appendU64(out, ownShipBodyId);
	appendF64(out, serverTimeScale);
	appendU64(out, joinGlobalPhysicsStep);
	return true;
}

bool readJoinAccept(const std::uint8_t* data,
                    const std::size_t len,
                    std::uint64_t& serverTickOut,
                    std::uint64_t& joinGlobalPhysicsStepOut,
                    std::vector<sim::AuthoritativeBody>& bodiesOut,
                    sim::BodyId& ownShipBodyIdOut,
                    double& serverTimeScaleOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	ownShipBodyIdOut = 0;
	serverTimeScaleOut = 1.0;
	joinGlobalPhysicsStepOut = 0;
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
	return p == end;
}

bool writeClientInput(const ClientInputPayload& in, std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::ClientInput);
	appendU32(out, in.seq);
	appendU8(out, in.thrustForward);
	appendU8(out, in.thrustReverse);
	appendF32(out, in.facingRadians);
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
	if (end - p < 2) {
		return false;
	}
	out.thrustForward = p[0];
	out.thrustReverse = p[1];
	p += 2;
	if (!readF32(p, end, out.facingRadians)) {
		return false;
	}
	return p == end;
}

bool writeShipState(const std::uint64_t serverTick,
                    const std::uint64_t globalPhysicsStep,
                    const sim::BodyId bodyId,
                    const double px,
                    const double py,
                    const double vx,
                    const double vy,
                    const float facing,
                    const std::uint8_t thrustForward,
                    const std::uint8_t thrustReverse,
                    std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::ShipState);
	appendU64(out, serverTick);
	appendU64(out, globalPhysicsStep);
	appendU64(out, bodyId);
	appendF64(out, px);
	appendF64(out, py);
	appendF64(out, vx);
	appendF64(out, vy);
	appendF32(out, facing);
	appendU8(out, thrustForward);
	appendU8(out, thrustReverse);
	return true;
}

bool readShipState(const std::uint8_t* data,
                   const std::size_t len,
                   std::uint64_t& tickOut,
                   std::uint64_t& globalPhysicsStepOut,
                   sim::BodyId& bodyIdOut,
                   double& px,
                   double& py,
                   double& vx,
                   double& vy,
                   float& facingOut,
                   std::uint8_t& thrustForwardOut,
                   std::uint8_t& thrustReverseOut) {
	const std::uint8_t* p = data;
	const std::uint8_t* end = data + len;
	std::uint8_t ver = 0;
	globalPhysicsStepOut = 0;
	if (!readHeader(p, end, MsgType::ShipState, ver)) {
		return false;
	}
	if (!readU64(p, end, tickOut) || !readU64(p, end, globalPhysicsStepOut) ||
	    !readU64(p, end, bodyIdOut)) {
		return false;
	}
	if (!readF64(p, end, px) || !readF64(p, end, py) || !readF64(p, end, vx) ||
	    !readF64(p, end, vy) || !readF32(p, end, facingOut)) {
		return false;
	}
	if (end - p < 2) {
		return false;
	}
	thrustForwardOut = p[0];
	thrustReverseOut = p[1];
	p += 2;
	return p == end;
}

bool writeWorldDynamicSnapshot(const std::uint64_t serverTick,
                               const std::uint64_t globalPhysicsStep,
                               const std::vector<sim::BodyId>& ids,
                               const double* px,
                               const double* py,
                               const double* vx,
                               const double* vy,
                               const std::size_t n,
                               std::vector<std::uint8_t>& out) {
	out.clear();
	writeHeader(out, MsgType::WorldDynamicSnapshot);
	appendU64(out, serverTick);
	appendU64(out, globalPhysicsStep);
	appendU32(out, static_cast<std::uint32_t>(n));
	for (std::size_t i = 0; i < n; ++i) {
		appendU64(out, ids[i]);
		appendF64(out, px[i]);
		appendF64(out, py[i]);
		appendF64(out, vx[i]);
		appendF64(out, vy[i]);
	}
	return true;
}

bool readWorldDynamicSnapshot(const std::uint8_t* data,
                              const std::size_t len,
                              std::uint64_t& tickOut,
                              std::uint64_t& globalPhysicsStepOut,
                              std::vector<sim::BodyId>& idsOut,
                              std::vector<double>& pxOut,
                              std::vector<double>& pyOut,
                              std::vector<double>& vxOut,
                              std::vector<double>& vyOut) {
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
	idsOut.resize(n);
	pxOut.resize(n);
	pyOut.resize(n);
	vxOut.resize(n);
	vyOut.resize(n);
	for (std::uint32_t i = 0; i < n; ++i) {
		if (!readU64(p, end, idsOut[i])) {
			return false;
		}
		if (!readF64(p, end, pxOut[i]) || !readF64(p, end, pyOut[i]) ||
		    !readF64(p, end, vxOut[i]) || !readF64(p, end, vyOut[i])) {
			return false;
		}
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

}  // namespace net
