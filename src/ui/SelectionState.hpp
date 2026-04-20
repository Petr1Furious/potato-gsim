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

	void applyMergeRemap(const std::vector<std::pair<sim::BodyId, sim::BodyId>>& remap);
	void validateAgainstEngine(const sim::SimulationEngine& engine);

	std::optional<sim::BodyId> pick(
	    const sim::SimulationEngine& engine,
	    const sf::Vector2f& worldPoint,
	    const std::function<float(const sf::Vector2f&)>& worldToScreenDistance,
	    std::optional<sim::BodyId> excludedBodyId,
	    float maxScreenDistancePx) const;

   private:
	std::optional<sim::BodyId> selectedId_;
};

}  // namespace ui
