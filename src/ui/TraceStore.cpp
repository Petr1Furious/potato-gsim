#include "ui/TraceStore.hpp"

#include <algorithm>
#include <utility>

namespace ui {

namespace {

std::pair<double, double> interpolateWorld(const std::deque<TraceStore::TracePoint>& trail,
                                           double timeSeconds) {
	if (trail.empty()) {
		return {0.0, 0.0};
	}
	if (timeSeconds <= trail.front().timeSeconds) {
		return {trail.front().worldX, trail.front().worldY};
	}
	if (timeSeconds >= trail.back().timeSeconds) {
		return {trail.back().worldX, trail.back().worldY};
	}

	const auto upper = std::lower_bound(
	    trail.begin(), trail.end(), timeSeconds,
	    [](const TraceStore::TracePoint& point, double t) { return point.timeSeconds < t; });
	if (upper == trail.begin()) {
		return {upper->worldX, upper->worldY};
	}
	if (upper == trail.end()) {
		return {trail.back().worldX, trail.back().worldY};
	}

	const auto lower = upper - 1;
	const double dt = upper->timeSeconds - lower->timeSeconds;
	if (dt <= 1e-12) {
		return {upper->worldX, upper->worldY};
	}
	const double alpha = std::clamp((timeSeconds - lower->timeSeconds) / dt, 0.0, 1.0);
	const double wx = lower->worldX + (upper->worldX - lower->worldX) * alpha;
	const double wy = lower->worldY + (upper->worldY - lower->worldY) * alpha;
	return {wx, wy};
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

void TraceStore::ingest(const std::vector<sim::BodySnapshot>& bodies, double timeSeconds) {
	if (!settings_.enabled) {
		return;
	}
	for (const sim::BodySnapshot& body : bodies) {
		auto& trail = traces_[body.id];
		trail.push_back(TracePoint{
		    .worldX = body.x,
		    .worldY = body.y,
		    .timeSeconds = timeSeconds,
		});
		while (trail.size() > settings_.maxPointsPerBody) {
			trail.pop_front();
		}
	}
}

void TraceStore::draw(sf::RenderWindow& window,
                      const std::optional<sim::BodySnapshot>& selectedBody,
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

	bool relativeMode = settings_.relative && selectedBody.has_value();
	const std::deque<TracePoint>* selectedTrail = nullptr;
	if (relativeMode) {
		const auto selectedIt =
		    selectedBody.has_value() ? traces_.find(selectedBody->id) : traces_.end();
		if (selectedIt != traces_.end() && !selectedIt->second.empty()) {
			selectedTrail = &selectedIt->second;
		} else {
			relativeMode = false;
		}
	}

	const double selX = selectedBody.has_value() ? selectedBody->x : 0.0;
	const double selY = selectedBody.has_value() ? selectedBody->y : 0.0;

	for (const auto& [id, trail] : traces_) {
		if (trail.size() < 2) {
			continue;
		}
		sf::VertexArray strip(sf::PrimitiveType::LineStrip, trail.size());
		const bool highlight = selectedBody.has_value() && selectedBody->id == id;
		for (std::size_t i = 0; i < trail.size(); ++i) {
			double wx = trail[i].worldX;
			double wy = trail[i].worldY;
			if (relativeMode && selectedTrail != nullptr) {
				const auto [sx, sy] = interpolateWorld(*selectedTrail, trail[i].timeSeconds);
				wx = wx - sx + selX;
				wy = wy - sy + selY;
			}
			strip[i].position = toLocal(wx, wy);
			strip[i].color =
			    highlight ? sf::Color(255, 240, 180, 180) : sf::Color(130, 160, 255, 95);
		}
		window.draw(strip);
	}
}

}  // namespace ui
