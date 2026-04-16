#include "ui/InputActions.hpp"

#include <SFML/Window/Keyboard.hpp>

#include <cmath>

namespace ui {

std::optional<Action> InputActions::mapKeyPress(sf::Keyboard::Key key, bool menuActive) const {
	switch (key) {
		case sf::Keyboard::Key::Escape:
			return Action::ToggleMenu;
		case sf::Keyboard::Key::F11:
			return Action::ToggleFullscreen;
		case sf::Keyboard::Key::Space:
			return menuActive ? std::nullopt : std::optional<Action>(Action::TogglePause);
		case sf::Keyboard::Key::M:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleMode);
		case sf::Keyboard::Key::R:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ResetView);
		case sf::Keyboard::Key::F:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleFollow);
		case sf::Keyboard::Key::L:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleTrails);
		case sf::Keyboard::Key::T:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleRelativeTrails);
		case sf::Keyboard::Key::P:
			return menuActive ? std::nullopt : std::optional<Action>(Action::TogglePredictions);
		case sf::Keyboard::Key::O:
			return menuActive ? std::nullopt
			                  : std::optional<Action>(Action::ToggleSelectedPrediction);
		case sf::Keyboard::Key::H:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleLabels);
		case sf::Keyboard::Key::F3:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleDebugInfo);
		case sf::Keyboard::Key::C:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleCreationTool);
		case sf::Keyboard::Key::V:
			return menuActive ? std::nullopt
			                  : std::optional<Action>(Action::ToggleRelativeCreationFrame);
		case sf::Keyboard::Key::N:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ToggleNegativeMass);
		case sf::Keyboard::Key::J:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ResetTimer);
		case sf::Keyboard::Key::Q:
			return menuActive ? std::nullopt : std::optional<Action>(Action::AdvancedResetFrame);
		case sf::Keyboard::Key::F5:
			return menuActive ? std::nullopt : std::optional<Action>(Action::RandomScenario);
		case sf::Keyboard::Key::F6:
			return menuActive ? std::nullopt : std::optional<Action>(Action::ClearScenario);
		case sf::Keyboard::Key::F7:
			return menuActive ? std::nullopt : std::optional<Action>(Action::PrevPreset);
		case sf::Keyboard::Key::F8:
			return menuActive ? std::nullopt : std::optional<Action>(Action::NextPreset);
		case sf::Keyboard::Key::F9:
			return menuActive ? std::nullopt : std::optional<Action>(Action::SaveWorld);
		case sf::Keyboard::Key::F10:
			return menuActive ? std::nullopt : std::optional<Action>(Action::LoadWorld);
		default:
			return std::nullopt;
	}
}

HoldAdjustments InputActions::computeHolds(double dtSeconds, bool enabled) const {
	HoldAdjustments out{};
	if (!enabled) {
		return out;
	}

	const double base = std::pow(1.9, dtSeconds);
	const double panSpeedPxPerSecond = 950.0;
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Period)) {
		out.timeScaleFactor *= base;
	}
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Comma)) {
		out.timeScaleFactor /= base;
	}

	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Equal)) {
		out.densityFactor *= base;
	}
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Add)) {
		out.densityFactor *= base;
	}
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Hyphen)) {
		out.densityFactor /= base;
	}
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Subtract)) {
		out.densityFactor /= base;
	}

	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::RBracket)) {
		out.predictionFactor *= base;
	}
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::LBracket)) {
		out.predictionFactor /= base;
	}

	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::A) ||
	    sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Left)) {
		out.panPixelsX += panSpeedPxPerSecond * dtSeconds;
	}
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::D) ||
	    sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Right)) {
		out.panPixelsX -= panSpeedPxPerSecond * dtSeconds;
	}
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::W) ||
	    sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Up)) {
		out.panPixelsY += panSpeedPxPerSecond * dtSeconds;
	}
	if (sf::Keyboard::isKeyPressed(sf::Keyboard::Key::S) ||
	    sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Down)) {
		out.panPixelsY -= panSpeedPxPerSecond * dtSeconds;
	}
	return out;
}

std::vector<std::string> InputActions::legendLines(bool menuActive) const {
	if (!menuActive) {
		return {};
	}
	return {
	    "Esc close menu | Up/Down select | Enter apply | Left/Right adjust",
	    "Space pause | M mode | F follow | C create | V rel-frame | N negative-mass",
	    "WASD/Arrows move | Left/Middle drag pan | Wheel zoom",
	    ",/. time-scale | -/= density | [/] prediction steps",
	    "L trails | T relative trails | P predictions | O selected prediction",
	    "H hover labels | F3 debug",
	    "F5 random | F6 clear | F7/F8 preset | F9 save | F10 load",
	};
}

}  // namespace ui
