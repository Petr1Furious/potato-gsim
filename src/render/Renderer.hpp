#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/Graphics.hpp>

namespace render {

class Renderer {
   public:
	explicit Renderer(sf::RenderWindow& window);

	void resetView();
	void onResize(const sf::Vector2u& size);
	void zoomAtPixel(const sf::Vector2i& pixel, float delta);
	void update(double dtSeconds);
	void panByPixels(const sf::Vector2i& pixelDelta);

	[[nodiscard]] sf::Vector2f screenToWorld(const sf::Vector2i& pixel) const;
	[[nodiscard]] sf::Vector2i worldToPixel(const sf::Vector2f& world) const;
	[[nodiscard]] float worldUnitsPerPixel() const;
	[[nodiscard]] const sf::View& view() const { return view_; }
	void setViewCenter(const sf::Vector2f& center) { view_.setCenter(center); }
	void setViewSize(const sf::Vector2f& size);

	void draw(const std::vector<sim::BodySnapshot>& bodies);
	void draw(const sim::SimulationEngine& engine);

   private:
	void enforceAspectRatio();

	sf::RenderWindow& window_;
	sf::View view_;
	sf::CircleShape circle_;
	sf::Vector2i zoomAnchorPixel_{0, 0};
	float zoomPendingSteps_ = 0.0f;
	bool hasZoomAnchor_ = false;
};

}  // namespace render
