#pragma once

#include "sim/BodyId.hpp"
#include "sim/CommandQueue.hpp"

#include <cstdint>
#include <deque>
#include <utility>
#include <vector>

namespace sim {
class SimulationEngine;
}

namespace net {

struct ShipThrustSample {
	sim::BodyId id = 0;
	double ax = 0.0;
	double ay = 0.0;
};

struct ReplayFrame {
	std::uint64_t globalPhysicsStepEnd = 0;
	std::vector<sim::AuthoritativeBody> bodiesAfter;
	std::vector<ShipThrustSample> thrustIntoThisStep;
};

/// Ring of full-world checkpoints + per-step ship thrust for rewind–replay.
class MpReplayBuffer {
   public:
	static constexpr std::size_t kMaxFrames = 512;

	void clear();
	void seedAfterJoin(std::uint64_t stepEnd, std::vector<sim::AuthoritativeBody> bodies);
	void recordAfterPhysicsStep(std::uint64_t stepEnd,
	                            std::vector<sim::AuthoritativeBody> bodiesAfter,
	                            std::vector<ShipThrustSample> thrustIntoThisStep);

	[[nodiscard]] bool findFrame(std::uint64_t stepEnd, ReplayFrame& out) const;
	[[nodiscard]] bool findLatestCheckpointAtOrBefore(std::uint64_t stepEnd,
	                                                  std::uint64_t& foundStepEndOut,
	                                                  ReplayFrame& out) const;

	/// Pop from back while last frame has stepEnd > keepThroughStep (removes speculative tail).
	void popFramesAfter(std::uint64_t keepThroughStep);

	[[nodiscard]] bool empty() const { return frames_.empty(); }
	[[nodiscard]] std::uint64_t newestStepEnd() const;

	/// Extract thrust sequences for (openBegin, closedEnd] before popping those frames.
	[[nodiscard]] bool extractThrustsBetween(
	    std::uint64_t openBegin,
	    std::uint64_t closedEnd,
	    std::vector<std::pair<std::uint64_t, std::vector<ShipThrustSample>>>& out) const;

   private:
	void trimSize();

	std::deque<ReplayFrame> frames_;
};

void copyBodiesToAuthoritative(const sim::SimulationEngine& engine,
                               std::vector<sim::AuthoritativeBody>& out);

void applyShipThrustSamples(sim::SimulationEngine& engine,
                            const std::vector<ShipThrustSample>& thrusts);

}  // namespace net
