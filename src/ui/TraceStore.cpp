#include "ui/TraceStore.hpp"

#include <algorithm>
#include <cmath>
#include <utility>

namespace ui {

namespace {

bool shouldCollapseMiddle(const TraceStore::TracePoint& a,
                          const TraceStore::TracePoint& b,
                          const TraceStore::TracePoint& c) {
	constexpr double kMinDtForTurnCheck = 1e-5;
	constexpr double kMaxStraightTurnRadians = 0.04;  // ~2.29 deg
	constexpr double kMinSegmentLength = 1e-9;
	const double kStraightCosThreshold = std::cos(kMaxStraightTurnRadians);

	if ((b.timeSeconds - a.timeSeconds) < kMinDtForTurnCheck ||
	    (c.timeSeconds - b.timeSeconds) < kMinDtForTurnCheck) {
		return true;
	}

	const double abx = b.worldX - a.worldX;
	const double aby = b.worldY - a.worldY;
	const double bcx = c.worldX - b.worldX;
	const double bcy = c.worldY - b.worldY;
	const double abLen = std::hypot(abx, aby);
	const double bcLen = std::hypot(bcx, bcy);
	if (abLen < kMinSegmentLength || bcLen < kMinSegmentLength) {
		return true;
	}

	const double cosAngle = std::clamp((abx * bcx + aby * bcy) / (abLen * bcLen), -1.0, 1.0);
	return cosAngle >= kStraightCosThreshold;
}

}  // namespace

void TraceStore::applyMergeRemap(const std::vector<std::pair<sim::BodyId, sim::BodyId>>& remap) {
	for (const auto& [from, to] : remap) {
		if (from == to) {
			continue;
		}
		auto fromIt = traces_.find(from);
		if (fromIt == traces_.end()) {
			continue;
		}
		traces_.erase(fromIt);
	}
}

void TraceStore::applyBodyDeletes(const std::vector<sim::BodyId>& ids) {
	for (const sim::BodyId id : ids) {
		traces_.erase(id);
	}
}

void TraceStore::ingest(const std::vector<sim::BodySnapshot>& bodies, double timeSeconds) {
	if (!settings_.enabled) {
		return;
	}
	for (const sim::BodySnapshot& body : bodies) {
		const TracePoint p{
		    .worldX = body.x,
		    .worldY = body.y,
		    .timeSeconds = timeSeconds,
		};
		auto& trail = traces_[body.id];
		if (settings_.simplify && trail.size() >= 2 &&
		    shouldCollapseMiddle(trail[trail.size() - 2], trail[trail.size() - 1], p)) {
			trail.back() = p;
		} else {
			trail.push_back(p);
		}
		while (trail.size() > settings_.maxPointsPerBody) {
			trail.pop_front();
		}
	}
}

void TraceStore::draw(sf::RenderWindow& window,
                      const std::optional<sim::BodySnapshot>& selectedBody,
                      const std::optional<sim::BodyId> relativeReferenceId,
                      const std::optional<std::pair<double, double>>& relativeReferenceAnchor,
                      double renderOriginX,
                      double renderOriginY,
                      bool useRenderOrigin) const {
	if (!settings_.enabled) {
		return;
	}

	const double ox = useRenderOrigin ? renderOriginX : 0.0;
	const double oy = useRenderOrigin ? renderOriginY : 0.0;

	auto toLocal = [&](double wx, double wy) {
		return sf::Vector2f(static_cast<float>(wx - ox), static_cast<float>(wy - oy));
	};

	const bool relativeMode = settings_.relative && relativeReferenceId.has_value() &&
	                          relativeReferenceAnchor.has_value();
	const auto refIt = (relativeMode ? traces_.find(*relativeReferenceId) : traces_.end());
	const bool haveRefTrail = (refIt != traces_.end() && !refIt->second.empty());
	const auto* refTrail = (haveRefTrail ? &refIt->second : nullptr);
	const double anchorX =
	    relativeReferenceAnchor.has_value() ? relativeReferenceAnchor->first : 0.0;
	const double anchorY =
	    relativeReferenceAnchor.has_value() ? relativeReferenceAnchor->second : 0.0;

	for (const auto& [id, trail] : traces_) {
		if (trail.size() < 2) {
			continue;
		}
		const bool highlight = selectedBody.has_value() && selectedBody->id == id;
		sf::VertexArray strip(sf::PrimitiveType::LineStrip);
		strip.resize(0);
		strip.clear();

		if (relativeMode && haveRefTrail && refTrail != nullptr) {
			if (refTrail->size() < 2) {
				continue;
			}
			const double refT0 = refTrail->front().timeSeconds;
			const double refT1 = refTrail->back().timeSeconds;
			std::size_t refIdx = 0;
			for (const TracePoint& p : trail) {
				const double t = p.timeSeconds;
				// Skip out-of-domain points to avoid first/last-segment artifacts.
				if (t < refT0 || t > refT1) {
					continue;
				}
				while (refIdx + 1 < refTrail->size() && (*refTrail)[refIdx + 1].timeSeconds < t) {
					++refIdx;
				}
				const TracePoint& a = (*refTrail)[refIdx];
				const TracePoint& b = (refIdx + 1 < refTrail->size()) ? (*refTrail)[refIdx + 1] : a;
				double rx = a.worldX;
				double ry = a.worldY;
				const double dt = b.timeSeconds - a.timeSeconds;
				if (dt > 1e-12) {
					const double alpha = std::clamp((t - a.timeSeconds) / dt, 0.0, 1.0);
					rx = a.worldX + (b.worldX - a.worldX) * alpha;
					ry = a.worldY + (b.worldY - a.worldY) * alpha;
				}

				const double wx = p.worldX - rx + anchorX;
				const double wy = p.worldY - ry + anchorY;
				strip.append(sf::Vertex{
				    toLocal(wx, wy),
				    highlight ? sf::Color(255, 240, 180, 180) : sf::Color(130, 160, 255, 95),
				});
			}
		} else {
			for (const TracePoint& p : trail) {
				double wx = p.worldX;
				double wy = p.worldY;
				if (relativeMode) {
					wx -= anchorX;
					wy -= anchorY;
				}
				strip.append(sf::Vertex{
				    toLocal(wx, wy),
				    highlight ? sf::Color(255, 240, 180, 180) : sf::Color(130, 160, 255, 95),
				});
			}
		}
		if (strip.getVertexCount() >= 2) {
			window.draw(strip);
		}
	}
}

}  // namespace ui
