#pragma once

#include <SFML/Window/Keyboard.hpp>

#include <bitset>

namespace ui {

/// Per-window held keys for continuous actions, indexed by `sf::Keyboard::Key`.
struct KeyboardChordState {
	using Key = sf::Keyboard::Key;

	std::bitset<sf::Keyboard::KeyCount> bits{};

	void clear() { bits.reset(); }

	void setDown(Key code, bool pressed) {
		if (code == Key::Unknown) {
			return;
		}
		const auto i = static_cast<unsigned>(code);
		if (i >= sf::Keyboard::KeyCount) {
			return;
		}
		bits[i] = pressed;
	}

	[[nodiscard]] bool down(Key code) const {
		if (code == Key::Unknown) {
			return false;
		}
		const auto i = static_cast<unsigned>(code);
		if (i >= sf::Keyboard::KeyCount) {
			return false;
		}
		return bits[i];
	}
};

}  // namespace ui
