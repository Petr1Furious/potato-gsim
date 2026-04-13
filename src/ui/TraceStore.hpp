#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/Graphics.hpp>

#include <deque>
#include <optional>
#include <unordered_map>
#include <utility>
#include <vector>

namespace ui {

class TraceStore {
   public:
	struct Settings {
		bool enabled = false;
		bool relative = false;
		std::size_t maxPointsPerBody = 240;
	};

	void setSettings(Settings settings) { settings_ = settings; }
	[[nodiscard]] const Settings& settings() const { return settings_; }

	void clear() { traces_.clear(); }
	void applyMergeRemap(const std::vector<std::pair<sim::BodyId, sim::BodyId>>& remap);
	void ingest(const std::vector<sim::BodySnapshot>& bodies);

	void draw(sf::RenderWindow& window, const std::optional<sim::BodySnapshot>& selectedBody) const;

   private:
	Settings settings_;
	std::unordered_map<sim::BodyId, std::deque<sf::Vector2f>> traces_;
};

}  // namespace ui
