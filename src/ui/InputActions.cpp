#include "ui/InputActions.hpp"

#include <SFML/Window/Keyboard.hpp>

namespace ui {

std::optional<Action> InputActions::mapKeyPress(sf::Keyboard::Key key, bool menuActive) const {
	switch (key) {
		case sf::Keyboard::Key::Escape:
			return Action::ToggleMenu;
		case sf::Keyboard::Key::F11:
			return Action::ToggleFullscreen;
		case sf::Keyboard::Key::R:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ResetView);
		case sf::Keyboard::Key::F:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleFollowMode);
		case sf::Keyboard::Key::L:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleTrails);
		case sf::Keyboard::Key::T:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleRelativeTrails);
		case sf::Keyboard::Key::P:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleShipPrediction);
		default:
			return std::nullopt;
	}
}

HoldAdjustments InputActions::computeHolds(const double dtSeconds,
                                           const bool enabled,
                                           const bool keyboardPanKeys,
                                           const KeyboardChordState& k) const {
	HoldAdjustments out{};
	if (!enabled) {
		return out;
	}

	const double panSpeedPxPerSecond = 950.0;

	if (keyboardPanKeys) {
		using K = sf::Keyboard::Key;
		if (k.down(K::A) || k.down(K::Left)) {
			out.panPixelsX += panSpeedPxPerSecond * dtSeconds;
		}
		if (k.down(K::D) || k.down(K::Right)) {
			out.panPixelsX -= panSpeedPxPerSecond * dtSeconds;
		}
		if (k.down(K::W) || k.down(K::Up)) {
			out.panPixelsY += panSpeedPxPerSecond * dtSeconds;
		}
		if (k.down(K::S) || k.down(K::Down)) {
			out.panPixelsY -= panSpeedPxPerSecond * dtSeconds;
		}
	}
	return out;
}

std::vector<std::string> InputActions::legendLines(bool menuActive) const {
	if (!menuActive) {
		return {};
	}
	return {
	    "Esc close menu | Up/Down select | Enter apply",
	    "F follow mode | L trails on/off | T trails relative/world",
	    "P ship prediction | R reset view | F11 fullscreen",
	    "W/Up thrust | M aim | Shift/Ctrl thrust | X 0% / Z 100%",
	    "Left/Middle drag pan | Wheel zoom",
	};
}

}  // namespace ui
