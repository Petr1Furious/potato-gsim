#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/System/Vector2.hpp>

#include <cstdint>
#include <optional>

namespace ui {

class CreationPauseStateMachine {
   public:
	[[nodiscard]] bool blocksPauseToggle() const { return lockActive_; }

	bool onCreationClickStart(const sf::Vector2i& clickPixel,
	                          bool creationEnabled,
	                          bool creationInProgress,
	                          sim::SimulationEngine& engine);
	bool onEscape(sim::SimulationEngine& engine);

	void tick(bool creationEnabled, bool creationInProgress, sim::SimulationEngine& engine);
	[[nodiscard]] std::optional<sf::Vector2i> consumeAutoStartPixel() {
		std::optional<sf::Vector2i> out = autoStartPixel_;
		autoStartPixel_.reset();
		return out;
	}

   private:
	void restorePaused(sim::SimulationEngine& engine);

	bool lockActive_ = false;
	bool awaitingStart_ = false;
	bool pausedBeforeCreation_ = false;
	std::optional<std::uint64_t> pauseAckToken_;
	std::optional<sf::Vector2i> pendingStartPixel_;
	std::optional<sf::Vector2i> autoStartPixel_;
};

}  // namespace ui
