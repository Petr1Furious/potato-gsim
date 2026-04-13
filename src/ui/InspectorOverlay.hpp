#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/Graphics.hpp>

#include <functional>
#include <optional>
#include <string>
#include <vector>

namespace ui {

class InspectorOverlay {
   public:
	InspectorOverlay();

	void drawWorldSelection(
	    sf::RenderWindow& window,
	    const std::optional<sim::BodySnapshot>& body,
	    const std::optional<sim::BodySnapshot>& referenceBody,
	    const std::vector<sf::Vector2f>& selectedPrediction,
	    const std::optional<sf::Vector2f>& creationVelocityTarget,
	    double arrowScale,
	    bool drawHighlightSquare,
	    bool showDistanceToReference,
	    const std::function<sf::Vector2i(const sf::Vector2f&)>& worldToPixel) const;

	void drawLabels(sf::RenderWindow& window,
	                const std::vector<sim::BodySnapshot>& bodies,
	                const std::optional<sim::BodyId>& selectedId,
	                const std::function<sf::Vector2i(const sf::Vector2f&)>& worldToPixel,
	                float maxScreenRadiusPx) const;

	void drawHudPanel(sf::RenderWindow& window,
	                  const std::vector<std::string>& lines,
	                  const std::vector<std::string>& legendLines,
	                  bool paused) const;

	[[nodiscard]] bool available() const { return fontReady_; }
	[[nodiscard]] const sf::Font* fontPtr() const { return fontReady_ ? &font_ : nullptr; }

   private:
	bool tryLoadFont();
	void drawArrow(sf::RenderWindow& window,
	               const sf::Vector2f& from,
	               const sf::Vector2f& to) const;
	void drawBodyInfoText(
	    sf::RenderWindow& window,
	    const sim::BodySnapshot& body,
	    const std::optional<sim::BodySnapshot>& referenceBody,
	    bool showDistance,
	    const std::function<sf::Vector2i(const sf::Vector2f&)>& worldToPixel) const;

	mutable sf::Font font_;
	bool fontReady_ = false;
};

}  // namespace ui
