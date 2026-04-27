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

	void drawWorldSelection(sf::RenderWindow& window,
	                        const std::optional<sim::BodySnapshot>& body,
	                        const std::optional<sim::BodySnapshot>& velocityReferenceBody,
	                        const std::optional<sim::BodySnapshot>& distanceReferenceBody,
	                        const std::vector<sf::Vector2f>& selectedPrediction,
	                        bool highlightPredictionEncounterEnd,
	                        const std::optional<sf::Vector2f>& creationVelocityTarget,
	                        double arrowScale,
	                        bool drawHighlightSquare,
	                        bool showDistanceToReference,
	                        const std::function<sf::Vector2f(double, double)>& worldToRenderLocal,
	                        const std::function<sf::Vector2i(double, double)>& worldToPixel) const;

	void drawLabels(sf::RenderWindow& window,
	                const std::vector<sim::BodySnapshot>& bodies,
	                const std::optional<sim::BodyId>& selectedId,
	                const std::function<sf::Vector2i(double, double)>& worldToPixel,
	                float maxScreenRadiusPx) const;

	void drawHudPanel(sf::RenderWindow& window,
	                  const std::vector<std::string>& lines,
	                  const std::vector<std::string>& legendLines,
	                  bool paused) const;

	/// Bottom-right thrust bar (0–100 %) when flying own ship in multiplayer.
	void drawShipThrustHud(sf::RenderWindow& window,
	                       int thrustPercent,
	                       float deltaVCurrentMps,
	                       float deltaVMaxMps,
	                       float shellReloadWallSeconds) const;

	/// Center-bottom banner when waiting to respawn (multiplayer).
	void drawRespawnCountdownBanner(sf::RenderWindow& window, double wallSecondsRemaining) const;

	[[nodiscard]] bool available() const { return fontReady_; }
	[[nodiscard]] const sf::Font* fontPtr() const { return fontReady_ ? &font_ : nullptr; }

   private:
	bool tryLoadFont();
	void drawArrow(sf::RenderWindow& window,
	               const sf::Vector2f& from,
	               const sf::Vector2f& to) const;
	void drawBodyInfoText(sf::RenderWindow& window,
	                      const sim::BodySnapshot& body,
	                      const std::optional<sim::BodySnapshot>& velocityReferenceBody,
	                      const std::optional<sim::BodySnapshot>& distanceReferenceBody,
	                      bool showDistance,
	                      const std::function<sf::Vector2i(double, double)>& worldToPixel) const;

	mutable sf::Font font_;
	bool fontReady_ = false;
};

}  // namespace ui
