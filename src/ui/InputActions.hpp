#pragma once

#include <SFML/Window/Keyboard.hpp>

#include <optional>
#include <string>
#include <vector>

namespace ui {

enum class Action {
	ToggleMenu,
	TogglePause,
	ToggleMode,
	ToggleFullscreen,
	ResetView,
	ToggleFollow,
	ToggleTrails,
	ToggleRelativeTrails,
	TogglePredictions,
	ToggleLabels,
	ToggleDebugInfo,
	ToggleCreationTool,
	ToggleRelativeCreationFrame,
	ToggleNegativeMass,
	ResetTimer,
	AdvancedResetFrame,
	SaveWorld,
	LoadWorld,
	RandomScenario,
	ClearScenario,
	NextPreset,
	PrevPreset
};

struct HoldAdjustments {
	double timeScaleFactor = 1.0;
	double densityFactor = 1.0;
	double predictionFactor = 1.0;
	double panPixelsX = 0.0;
	double panPixelsY = 0.0;
};

class InputActions {
   public:
	[[nodiscard]] std::optional<Action> mapKeyPress(sf::Keyboard::Key key, bool menuActive) const;
	[[nodiscard]] HoldAdjustments computeHolds(double dtSeconds, bool enabled) const;

	[[nodiscard]] std::vector<std::string> legendLines(bool menuActive) const;
};

}  // namespace ui
