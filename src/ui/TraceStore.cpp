#include "ui/TraceStore.hpp"

#include <algorithm>

namespace ui {

sf::Vector2f TraceStore::interpolatePosition(const std::deque<TracePoint>& trail,
                                             double timeSeconds) {
	if (trail.empty()) {
		return sf::Vector2f(0.0f, 0.0f);
	}
	if (timeSeconds <= trail.front().timeSeconds) {
		return trail.front().worldPos;
	}
	if (timeSeconds >= trail.back().timeSeconds) {
		return trail.back().worldPos;
	}

	const auto upper =
	    std::lower_bound(trail.begin(), trail.end(), timeSeconds,
	                     [](const TracePoint& point, double t) { return point.timeSeconds < t; });
	if (upper == trail.begin()) {
		return upper->worldPos;
	}
	if (upper == trail.end()) {
		return trail.back().worldPos;
	}

	const auto lower = upper - 1;
	const double dt = upper->timeSeconds - lower->timeSeconds;
	if (dt <= 1e-12) {
		return upper->worldPos;
	}
	const double alpha = std::clamp((timeSeconds - lower->timeSeconds) / dt, 0.0, 1.0);
	return sf::Vector2f(
	    static_cast<float>(lower->worldPos.x + (upper->worldPos.x - lower->worldPos.x) * alpha),
	    static_cast<float>(lower->worldPos.y + (upper->worldPos.y - lower->worldPos.y) * alpha));
}

void TraceStore::applyMergeRemap(const std::vector<std::pair<sim::BodyId, sim::BodyId>>& remap) {
	for (const auto& [from, to] : remap) {
		if (from == to) {
			continue;
		}
		auto fromIt = traces_.find(from);
		if (fromIt == traces_.end()) {
			continue;
		}
		auto& target = traces_[to];
		target.insert(target.end(), fromIt->second.begin(), fromIt->second.end());
		std::stable_sort(
		    target.begin(), target.end(),
		    [](const TracePoint& a, const TracePoint& b) { return a.timeSeconds < b.timeSeconds; });
		while (target.size() > settings_.maxPointsPerBody) {
			target.pop_front();
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
		    .worldPos = sf::Vector2f(static_cast<float>(body.x), static_cast<float>(body.y)),
		    .timeSeconds = timeSeconds,
		});
		while (trail.size() > settings_.maxPointsPerBody) {
			trail.pop_front();
		}
	}
}

void TraceStore::draw(sf::RenderWindow& window,
                      const std::optional<sim::BodySnapshot>& selectedBody) const {
	if (!settings_.enabled) {
		return;
	}

	bool relativeMode = settings_.relative && selectedBody.has_value();
	sf::Vector2f selectedCurrent(0.0f, 0.0f);
	const std::deque<TracePoint>* selectedTrail = nullptr;
	if (relativeMode) {
		selectedCurrent =
		    sf::Vector2f(static_cast<float>(selectedBody->x), static_cast<float>(selectedBody->y));
		const auto selectedIt = traces_.find(selectedBody->id);
		if (selectedIt != traces_.end() && !selectedIt->second.empty()) {
			selectedTrail = &selectedIt->second;
		} else {
			relativeMode = false;
		}
	}

	for (const auto& [id, trail] : traces_) {
		if (trail.size() < 2) {
			continue;
		}
		sf::VertexArray strip(sf::PrimitiveType::LineStrip, trail.size());
		const bool highlight = selectedBody.has_value() && selectedBody->id == id;
		for (std::size_t i = 0; i < trail.size(); ++i) {
			sf::Vector2f drawPos = trail[i].worldPos;
			if (relativeMode && selectedTrail != nullptr) {
				const sf::Vector2f selectedAtTime =
				    interpolatePosition(*selectedTrail, trail[i].timeSeconds);
				drawPos = drawPos - selectedAtTime + selectedCurrent;
			}
			strip[i].position = drawPos;
			strip[i].color =
			    highlight ? sf::Color(255, 240, 180, 180) : sf::Color(130, 160, 255, 95);
		}
		window.draw(strip);
	}
}

}  // namespace ui
