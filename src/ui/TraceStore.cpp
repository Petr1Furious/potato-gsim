#include "ui/TraceStore.hpp"

#include <algorithm>

namespace ui {

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
		while (target.size() > settings_.maxPointsPerBody) {
			target.pop_front();
		}
		traces_.erase(fromIt);
	}
}

void TraceStore::ingest(const std::vector<sim::BodySnapshot>& bodies) {
	if (!settings_.enabled) {
		return;
	}
	for (const sim::BodySnapshot& body : bodies) {
		auto& trail = traces_[body.id];
		trail.emplace_back(static_cast<float>(body.x), static_cast<float>(body.y));
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

	sf::Vector2f base(0.0f, 0.0f);
	if (settings_.relative && selectedBody.has_value()) {
		base =
		    sf::Vector2f(static_cast<float>(selectedBody->x), static_cast<float>(selectedBody->y));
	}

	for (const auto& [id, trail] : traces_) {
		if (trail.size() < 2) {
			continue;
		}
		sf::VertexArray strip(sf::PrimitiveType::LineStrip, trail.size());
		const bool highlight = selectedBody.has_value() && selectedBody->id == id;
		for (std::size_t i = 0; i < trail.size(); ++i) {
			strip[i].position = trail[i] - base;
			strip[i].color =
			    highlight ? sf::Color(255, 240, 180, 180) : sf::Color(130, 160, 255, 95);
		}
		window.draw(strip);
	}
}

}  // namespace ui
