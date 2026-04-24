#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/Graphics.hpp>

#include <optional>
#include <unordered_map>

namespace render {

class Renderer {
   public:
	explicit Renderer(sf::RenderWindow& window);

	void resetView();
	void onResize(const sf::Vector2u& size);
	void zoomAtPixel(const sf::Vector2i& pixel, float delta);
	void update(double dtSeconds);
	void panByPixels(const sf::Vector2i& pixelDelta);

	struct WorldCoordsD {
		double x = 0.0;
		double y = 0.0;
	};

	[[nodiscard]] WorldCoordsD screenToWorldD(const sf::Vector2i& pixel) const;
	[[nodiscard]] sf::Vector2f screenToWorld(const sf::Vector2i& pixel) const;
	[[nodiscard]] sf::Vector2i worldToPixel(const sf::Vector2f& world) const;
	[[nodiscard]] sf::Vector2i worldToPixelD(double worldX, double worldY) const;
	[[nodiscard]] sf::Vector2f worldToRenderLocal(double worldX, double worldY) const;

	void worldRenderingOrigin(double& outX, double& outY, bool& outUsesOrigin) const {
		outUsesOrigin = useWorldOrigin_;
		outX = worldOriginX_;
		outY = worldOriginY_;
	}

	[[nodiscard]] WorldCoordsD cameraWorldCenter() const { return {cameraWorldX_, cameraWorldY_}; }

	void setWorldOriginForRendering(double ox, double oy);
	void clearWorldRenderingOrigin();
	void setCameraWorldCenterDouble(double cx, double cy);

	[[nodiscard]] float worldUnitsPerPixel() const;
	[[nodiscard]] const sf::View& view() const { return view_; }
	void setViewSize(const sf::Vector2f& size);

	/// When `multiplayerShipFacings` is set, each listed body is drawn as an oriented ship (others
	/// use `otherShip*` colors); own ship uses `playerFacingRadians` when provided.
	void draw(const std::vector<sim::BodySnapshot>& bodies,
	          std::optional<sim::BodyId> playerShipId = std::nullopt,
	          std::optional<float> playerFacingRadians = std::nullopt,
	          const std::unordered_map<sim::BodyId, float>* multiplayerShipFacings = nullptr);
	void draw(const sim::SimulationEngine& engine);

   private:
	void enforceAspectRatio();
	void refreshCameraWorldFromView();

	sf::RenderWindow& window_;
	sf::View view_;
	sf::CircleShape circle_;
	sf::Vector2i zoomAnchorPixel_{0, 0};
	float zoomPendingSteps_ = 0.0f;
	bool hasZoomAnchor_ = false;

	double worldOriginX_{0.0};
	double worldOriginY_{0.0};
	bool useWorldOrigin_{false};
	double cameraWorldX_{0.0};
	double cameraWorldY_{0.0};
};

}  // namespace render
