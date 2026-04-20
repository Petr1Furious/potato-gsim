#pragma once

#include <SFML/Window/Keyboard.hpp>

#include <optional>
#include <string>
#include <vector>

namespace ui {

enum class Action {
	ToggleMenu,
	ToggleFullscreen,
	ResetView,
	ToggleFollowMode,
	ToggleTrails,
	ToggleRelativeTrails,
	ToggleShipPrediction
};

struct HoldAdjustments {
	double panPixelsX = 0.0;
	double panPixelsY = 0.0;
};

class InputActions {
   public:
	[[nodiscard]] std::optional<Action> mapKeyPress(sf::Keyboard::Key key, bool menuActive) const;
	[[nodiscard]] HoldAdjustments computeHolds(double dtSeconds,
	                                           bool enabled,
	                                           bool keyboardPanKeys = true) const;

	[[nodiscard]] std::vector<std::string> legendLines(bool menuActive) const;
};

}  // namespace ui
