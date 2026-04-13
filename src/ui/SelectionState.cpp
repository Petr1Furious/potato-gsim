#include "ui/SelectionState.hpp"

#include <algorithm>
#include <cmath>

namespace ui {

void SelectionState::applyMergeRemap(
    const std::vector<std::pair<sim::BodyId, sim::BodyId>>& remap) {
	if (!selectedId_.has_value()) {
		return;
	}
	sim::BodyId current = *selectedId_;
	bool changed = true;
	while (changed) {
		changed = false;
		for (const auto& [from, to] : remap) {
			if (from == current) {
				current = to;
				changed = true;
			}
		}
	}
	selectedId_ = current;
}

void SelectionState::validateAgainstEngine(const sim::SimulationEngine& engine) {
	if (!selectedId_.has_value()) {
		return;
	}
	if (!engine.bodyById(*selectedId_).has_value()) {
		selectedId_.reset();
	}
}

std::optional<sim::BodyId> SelectionState::pick(
    const sim::SimulationEngine& engine,
    const sf::Vector2f& worldPoint,
    const std::function<float(const sf::Vector2f&)>& worldToScreenDistance,
    float maxScreenDistancePx) const {
	std::optional<sim::BodyId> bestId;
	float bestScreenDistance = maxScreenDistancePx;
	double bestMass = -1.0;

	engine.withReadSnapshot([&](const sim::BodyState& state, const sim::IdIndexMap& idMap) {
		for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(state.size()); ++i) {
			const sf::Vector2f pos(static_cast<float>(state.posX[i]),
			                       static_cast<float>(state.posY[i]));
			const float distPx = worldToScreenDistance(pos - worldPoint);
			const float radiusPx =
			    worldToScreenDistance(sf::Vector2f(static_cast<float>(state.radius[i]), 0.0f));
			const float surfaceDist = std::max(0.0f, distPx - radiusPx);
			if (surfaceDist > maxScreenDistancePx) {
				continue;
			}

			const double massAbs = std::abs(state.mass[i]);
			constexpr float kDistanceBiasPx = 2.0f;
			const bool betterDistance = surfaceDist + kDistanceBiasPx < bestScreenDistance;
			const bool closeEnoughDistance =
			    std::abs(surfaceDist - bestScreenDistance) <= kDistanceBiasPx;
			const bool betterMassTieBreak = closeEnoughDistance && (massAbs > bestMass);
			if (betterDistance || betterMassTieBreak) {
				bestScreenDistance = surfaceDist;
				bestMass = massAbs;
				bestId = idMap.idAtDenseIndex(i);
			}
		}
	});

	return bestId;
}

}  // namespace ui
