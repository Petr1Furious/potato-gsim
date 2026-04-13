#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/System/Vector2.hpp>

#include <functional>
#include <optional>
#include <utility>
#include <vector>

namespace ui {

class SelectionState {
   public:
	void clear() { selectedId_.reset(); }

	void setSelected(sim::BodyId id) { selectedId_ = id; }

	[[nodiscard]] std::optional<sim::BodyId> selectedId() const { return selectedId_; }

	void toggleFollow() { followEnabled_ = !followEnabled_; }

	void setFollow(bool enabled) { followEnabled_ = enabled; }

	[[nodiscard]] bool followEnabled() const { return followEnabled_; }

	void applyMergeRemap(const std::vector<std::pair<sim::BodyId, sim::BodyId>>& remap);
	void validateAgainstEngine(const sim::SimulationEngine& engine);

	std::optional<sim::BodyId> pick(
	    const sim::SimulationEngine& engine,
	    const sf::Vector2f& worldPoint,
	    const std::function<float(const sf::Vector2f&)>& worldToScreenDistance,
	    float maxScreenDistancePx) const;

   private:
	std::optional<sim::BodyId> selectedId_;
	bool followEnabled_ = false;
};

}  // namespace ui
