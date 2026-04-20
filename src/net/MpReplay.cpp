#include "net/MpReplay.hpp"

#include "sim/SimulationEngine.hpp"

namespace net {

void MpReplayBuffer::clear() {
	frames_.clear();
}

void MpReplayBuffer::seedAfterJoin(const std::uint64_t stepEnd,
                                   std::vector<sim::AuthoritativeBody> bodies) {
	frames_.clear();
	ReplayFrame f{};
	f.globalPhysicsStepEnd = stepEnd;
	f.bodiesAfter = std::move(bodies);
	f.thrustIntoThisStep.clear();
	frames_.push_back(std::move(f));
}

void MpReplayBuffer::recordAfterPhysicsStep(const std::uint64_t stepEnd,
                                            std::vector<sim::AuthoritativeBody> bodiesAfter,
                                            std::vector<ShipThrustSample> thrustIntoThisStep) {
	if (!frames_.empty() && stepEnd <= frames_.back().globalPhysicsStepEnd) {
		while (!frames_.empty() && frames_.back().globalPhysicsStepEnd >= stepEnd) {
			frames_.pop_back();
		}
	}
	ReplayFrame f{};
	f.globalPhysicsStepEnd = stepEnd;
	f.bodiesAfter = std::move(bodiesAfter);
	f.thrustIntoThisStep = std::move(thrustIntoThisStep);
	frames_.push_back(std::move(f));
	trimSize();
}

bool MpReplayBuffer::findFrame(const std::uint64_t stepEnd, ReplayFrame& out) const {
	for (auto it = frames_.rbegin(); it != frames_.rend(); ++it) {
		if (it->globalPhysicsStepEnd == stepEnd) {
			out = *it;
			return true;
		}
	}
	return false;
}

bool MpReplayBuffer::findLatestCheckpointAtOrBefore(const std::uint64_t stepEnd,
                                                    std::uint64_t& foundStepEndOut,
                                                    ReplayFrame& out) const {
	for (auto it = frames_.rbegin(); it != frames_.rend(); ++it) {
		if (it->globalPhysicsStepEnd <= stepEnd && !it->bodiesAfter.empty()) {
			foundStepEndOut = it->globalPhysicsStepEnd;
			out = *it;
			return true;
		}
	}
	return false;
}

void MpReplayBuffer::popFramesAfter(const std::uint64_t keepThroughStep) {
	while (!frames_.empty() && frames_.back().globalPhysicsStepEnd > keepThroughStep) {
		frames_.pop_back();
	}
}

std::uint64_t MpReplayBuffer::newestStepEnd() const {
	return frames_.empty() ? 0u : frames_.back().globalPhysicsStepEnd;
}

bool MpReplayBuffer::extractThrustsBetween(
    const std::uint64_t openBegin,
    const std::uint64_t closedEnd,
    std::vector<std::pair<std::uint64_t, std::vector<ShipThrustSample>>>& out) const {
	out.clear();
	for (std::uint64_t g = openBegin + 1; g <= closedEnd; ++g) {
		ReplayFrame fr{};
		if (!findFrame(g, fr)) {
			return false;
		}
		out.push_back({g, fr.thrustIntoThisStep});
	}
	return true;
}

void MpReplayBuffer::trimSize() {
	while (frames_.size() > kMaxFrames) {
		frames_.pop_front();
	}
}

void copyBodiesToAuthoritative(const sim::SimulationEngine& engine,
                               std::vector<sim::AuthoritativeBody>& out) {
	std::vector<sim::BodySnapshot> snaps;
	engine.copyBodies(snaps);
	out.clear();
	out.reserve(snaps.size());
	for (const sim::BodySnapshot& s : snaps) {
		out.push_back(sim::AuthoritativeBody{
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
}

void applyShipThrustSamples(sim::SimulationEngine& engine,
                            const std::vector<ShipThrustSample>& thrusts) {
	for (const ShipThrustSample& t : thrusts) {
		engine.setShipThrustAccelWorld(t.id, t.ax, t.ay);
	}
}

}  // namespace net
