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

void SelectionState::validateAgainstBodies(const std::vector<sim::BodySnapshot>& bodies) {
	if (!selectedId_.has_value()) {
		return;
	}
	const sim::BodyId want = *selectedId_;
	const bool found =
	    std::find_if(bodies.begin(), bodies.end(),
	                 [want](const sim::BodySnapshot& b) { return b.id == want; }) != bodies.end();
	if (!found) {
		selectedId_.reset();
	}
}

std::optional<sim::BodyId> SelectionState::pick(
    const sim::SimulationEngine& engine,
    const sf::Vector2f& worldPoint,
    const std::function<float(const sf::Vector2f&)>& worldToScreenDistance,
    const std::optional<sim::BodyId> excludedBodyId,
    float maxScreenDistancePx) const {
	std::optional<sim::BodyId> bestId;
	float bestScreenDistance = maxScreenDistancePx;
	double bestMass = -1.0;

	engine.withReadSnapshot([&](const sim::BodyState& state, const sim::IdIndexMap& idMap) {
		for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(state.size()); ++i) {
			const sim::BodyId currentId = idMap.idAtDenseIndex(i);
			if (excludedBodyId.has_value() && currentId == *excludedBodyId) {
				continue;
			}
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
				bestId = currentId;
			}
		}
	});

	return bestId;
}

std::optional<sim::BodyId> SelectionState::pickFromBodies(
    const std::vector<sim::BodySnapshot>& bodies,
    const sf::Vector2f& worldPoint,
    const std::function<float(const sf::Vector2f&)>& worldToScreenDistance,
    const std::optional<sim::BodyId> excludedBodyId,
    const float maxScreenDistancePx) const {
	std::optional<sim::BodyId> bestId;
	float bestScreenDistance = maxScreenDistancePx;
	double bestMass = -1.0;

	for (const sim::BodySnapshot& b : bodies) {
		if (excludedBodyId.has_value() && b.id == *excludedBodyId) {
			continue;
		}
		const sf::Vector2f pos(static_cast<float>(b.x), static_cast<float>(b.y));
		const float distPx = worldToScreenDistance(pos - worldPoint);
		const float radiusPx =
		    worldToScreenDistance(sf::Vector2f(static_cast<float>(b.radius), 0.0f));
		const float surfaceDist = std::max(0.0f, distPx - radiusPx);
		if (surfaceDist > maxScreenDistancePx) {
			continue;
		}

		const double massAbs = std::abs(b.mass);
		constexpr float kDistanceBiasPx = 2.0f;
		const bool betterDistance = surfaceDist + kDistanceBiasPx < bestScreenDistance;
		const bool closeEnoughDistance =
		    std::abs(surfaceDist - bestScreenDistance) <= kDistanceBiasPx;
		const bool betterMassTieBreak = closeEnoughDistance && (massAbs > bestMass);
		if (betterDistance || betterMassTieBreak) {
			bestScreenDistance = surfaceDist;
			bestMass = massAbs;
			bestId = b.id;
		}
	}

	return bestId;
}

}  // namespace ui
