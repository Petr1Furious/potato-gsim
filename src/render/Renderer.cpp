#include "render/Renderer.hpp"
#include "net/MpConstants.hpp"

#include <algorithm>
#include <cmath>

namespace render {

namespace {

void applyZoomStep(sf::RenderWindow& window,
                   sf::View& view,
                   const sf::Vector2i& pixel,
                   float steps) {
	if (steps == 0.0f) {
		return;
	}
	const sf::Vector2f before = window.mapPixelToCoords(pixel, view);
	const float factor = std::pow(1.1f, -steps);
	view.zoom(factor);
	const sf::Vector2f after = window.mapPixelToCoords(pixel, view);
	view.move(before - after);
}

}  // namespace

Renderer::Renderer(sf::RenderWindow& window) : window_(window), view_(window.getDefaultView()) {
	circle_.setFillColor(sf::Color(180, 220, 255));
	enforceAspectRatio();
	lastWindowSize_ = window_.getSize();
	refreshCameraWorldFromView();
}

void Renderer::refreshCameraWorldFromView() {
	const sf::Vector2f lc = view_.getCenter();
	const double ox = useWorldOrigin_ ? worldOriginX_ : 0.0;
	const double oy = useWorldOrigin_ ? worldOriginY_ : 0.0;
	cameraWorldX_ = static_cast<double>(lc.x) + ox;
	cameraWorldY_ = static_cast<double>(lc.y) + oy;
}

void Renderer::setWorldOriginForRendering(double ox, double oy) {
	useWorldOrigin_ = true;
	worldOriginX_ = ox;
	worldOriginY_ = oy;
	view_.setCenter(sf::Vector2f(static_cast<float>(cameraWorldX_ - ox),
	                             static_cast<float>(cameraWorldY_ - oy)));
}

void Renderer::clearWorldRenderingOrigin() {
	useWorldOrigin_ = false;
	worldOriginX_ = worldOriginY_ = 0.0;
	view_.setCenter(
	    sf::Vector2f(static_cast<float>(cameraWorldX_), static_cast<float>(cameraWorldY_)));
}

void Renderer::setCameraWorldCenterDouble(double cx, double cy) {
	cameraWorldX_ = cx;
	cameraWorldY_ = cy;
	const double ox = useWorldOrigin_ ? worldOriginX_ : 0.0;
	const double oy = useWorldOrigin_ ? worldOriginY_ : 0.0;
	view_.setCenter(sf::Vector2f(static_cast<float>(cx - ox), static_cast<float>(cy - oy)));
}

void Renderer::resetView() {
	view_ = window_.getDefaultView();
	enforceAspectRatio();
	useWorldOrigin_ = false;
	worldOriginX_ = worldOriginY_ = 0.0;
	lastWindowSize_ = window_.getSize();
	refreshCameraWorldFromView();
}

void Renderer::onResize(const sf::Vector2u& size) {
	if (size.x == 0 || size.y == 0) {
		return;
	}
	enforceAspectRatio();
	lastWindowSize_ = size;
}

void Renderer::setViewSize(const sf::Vector2f& size) {
	view_.setSize(size);
	enforceAspectRatio();
	lastWindowSize_ = window_.getSize();
}

void Renderer::enforceAspectRatio() {
	const sf::Vector2u win = window_.getSize();
	if (win.x == 0 || win.y == 0) {
		return;
	}
	const float windowAspect = static_cast<float>(win.x) / static_cast<float>(win.y);
	sf::Vector2f size = view_.getSize();
	if (size.x <= 0.0f || size.y <= 0.0f) {
		size = sf::Vector2f(1.0f, 1.0f);
	}
	const float widthFromHeight = size.y * windowAspect;
	if (widthFromHeight >= size.x) {
		size.x = widthFromHeight;
	} else {
		size.y = size.x / windowAspect;
	}
	view_.setSize(size);
	window_.setView(view_);
	refreshCameraWorldFromView();
}

void Renderer::zoomAtPixel(const sf::Vector2i& pixel, float delta) {
	if (delta == 0.0f) {
		return;
	}
	zoomAnchorPixel_ = pixel;
	zoomPendingSteps_ += delta;
	hasZoomAnchor_ = true;
}

void Renderer::update(double dtSeconds) {
	if (!hasZoomAnchor_ || std::abs(zoomPendingSteps_) < 1e-4f) {
		return;
	}
	const float dt = static_cast<float>(std::clamp(dtSeconds, 0.0, 0.1));
	const float smoothing = std::clamp(12.0f * dt, 0.0f, 1.0f);
	const float appliedSteps = zoomPendingSteps_ * smoothing;
	zoomPendingSteps_ -= appliedSteps;
	applyZoomStep(window_, view_, zoomAnchorPixel_, appliedSteps);
	refreshCameraWorldFromView();
}

void Renderer::panByPixels(const sf::Vector2i& pixelDelta) {
	const float scaleX = view_.getSize().x / static_cast<float>(window_.getSize().x);
	const float scaleY = view_.getSize().y / static_cast<float>(window_.getSize().y);
	view_.move(sf::Vector2f(-pixelDelta.x * scaleX, -pixelDelta.y * scaleY));
	refreshCameraWorldFromView();
}

Renderer::WorldCoordsD Renderer::screenToWorldD(const sf::Vector2i& pixel) const {
	const sf::Vector2f local = window_.mapPixelToCoords(pixel, view_);
	const double ox = useWorldOrigin_ ? worldOriginX_ : 0.0;
	const double oy = useWorldOrigin_ ? worldOriginY_ : 0.0;
	return {static_cast<double>(local.x) + ox, static_cast<double>(local.y) + oy};
}

sf::Vector2f Renderer::screenToWorld(const sf::Vector2i& pixel) const {
	const WorldCoordsD w = screenToWorldD(pixel);
	return {static_cast<float>(w.x), static_cast<float>(w.y)};
}

sf::Vector2i Renderer::worldToPixelD(double worldX, double worldY) const {
	const double ox = useWorldOrigin_ ? worldOriginX_ : 0.0;
	const double oy = useWorldOrigin_ ? worldOriginY_ : 0.0;
	const float lx = static_cast<float>(worldX - ox);
	const float ly = static_cast<float>(worldY - oy);
	return window_.mapCoordsToPixel(sf::Vector2f(lx, ly), view_);
}

sf::Vector2i Renderer::worldToPixel(const sf::Vector2f& world) const {
	return worldToPixelD(static_cast<double>(world.x), static_cast<double>(world.y));
}

sf::Vector2f Renderer::worldToRenderLocal(double worldX, double worldY) const {
	const double ox = useWorldOrigin_ ? worldOriginX_ : 0.0;
	const double oy = useWorldOrigin_ ? worldOriginY_ : 0.0;
	return {static_cast<float>(worldX - ox), static_cast<float>(worldY - oy)};
}

float Renderer::worldUnitsPerPixel() const {
	return view_.getSize().x / std::max(1.0f, static_cast<float>(window_.getSize().x));
}

namespace {

void drawOrientedShip(sf::RenderWindow& window,
                      const sf::Vector2f& pos,
                      float facing,
                      float shipWorldRadius,
                      float outlineThicknessWorld,
                      const sf::Color& fill,
                      const sf::Color& outline,
                      const sf::Color& headingLine) {
	const sf::Vector2f tip(pos.x + std::cos(facing) * shipWorldRadius * 1.8f,
	                       pos.y + std::sin(facing) * shipWorldRadius * 1.8f);
	const sf::Vector2f back(pos.x - std::cos(facing) * shipWorldRadius * 1.0f,
	                        pos.y - std::sin(facing) * shipWorldRadius * 1.0f);
	const sf::Vector2f left(back.x + std::cos(facing + 1.5707963f) * shipWorldRadius * 0.9f,
	                        back.y + std::sin(facing + 1.5707963f) * shipWorldRadius * 0.9f);
	const sf::Vector2f right(back.x + std::cos(facing - 1.5707963f) * shipWorldRadius * 0.9f,
	                         back.y + std::sin(facing - 1.5707963f) * shipWorldRadius * 0.9f);

	sf::VertexArray heading(sf::PrimitiveType::Lines, 2);
	heading[0].position = pos;
	heading[0].color = headingLine;
	heading[1].position = tip;
	heading[1].color = headingLine;
	window.draw(heading);

	sf::ConvexShape ship(3);
	ship.setPoint(0, tip);
	ship.setPoint(1, right);
	ship.setPoint(2, left);
	ship.setFillColor(fill);
	ship.setOutlineColor(outline);
	ship.setOutlineThickness(outlineThicknessWorld);
	window.draw(ship);
}

[[nodiscard]] bool isMpShellBody(const sim::BodySnapshot& b) {
	constexpr double eps = 1e-6;
	return std::abs(b.mass - net::kShellMass) < eps && std::abs(b.radius - net::kShellRadius) < eps;
}

void drawShellWithBlast(sf::RenderWindow& window, const sf::Vector2f& pos, float shellRadiusWorld) {
	sf::CircleShape blast(static_cast<float>(net::kShellExplosionRadius));
	blast.setOrigin(sf::Vector2f(static_cast<float>(net::kShellExplosionRadius),
	                             static_cast<float>(net::kShellExplosionRadius)));
	blast.setPosition(pos);
	blast.setFillColor(sf::Color(255, 120, 90, 52));
	blast.setOutlineColor(sf::Color(255, 200, 150, 130));
	blast.setOutlineThickness(1.0f);
	window.draw(blast);

	sf::CircleShape core(shellRadiusWorld);
	core.setOrigin(sf::Vector2f(shellRadiusWorld, shellRadiusWorld));
	core.setPosition(pos);
	core.setFillColor(sf::Color(255, 200, 160, 235));
	window.draw(core);
}

}  // namespace

void Renderer::draw(const std::vector<sim::BodySnapshot>& bodies,
                    const std::optional<sim::BodyId> playerShipId,
                    const std::optional<float> playerFacingRadians,
                    const std::unordered_map<sim::BodyId, float>* multiplayerShipFacings) {
	window_.setView(view_);
	window_.clear(sf::Color(8, 10, 16));

	const double ox = useWorldOrigin_ ? worldOriginX_ : 0.0;
	const double oy = useWorldOrigin_ ? worldOriginY_ : 0.0;

	const float pixelsPerWorld = 1.0f / worldUnitsPerPixel();
	const sf::Vector2f viewCenter = view_.getCenter();
	const sf::Vector2f viewSize = view_.getSize();
	const float viewMinX = viewCenter.x - viewSize.x * 0.5f;
	const float viewMaxX = viewCenter.x + viewSize.x * 0.5f;
	const float viewMinY = viewCenter.y - viewSize.y * 0.5f;
	const float viewMaxY = viewCenter.y + viewSize.y * 0.5f;
	sf::VertexArray points(sf::PrimitiveType::Points);

	for (const sim::BodySnapshot& body : bodies) {
		const bool isPlayer = playerShipId.has_value() && body.id == *playerShipId;
		const bool isShell = isMpShellBody(body);
		const float radiusPx = static_cast<float>(body.radius) * pixelsPerWorld;
		const float px = static_cast<float>(body.x - ox);
		const float py = static_cast<float>(body.y - oy);
		const sf::Vector2f pos(px, py);
		const float worldRadius = static_cast<float>(body.radius);
		if (pos.x + worldRadius < viewMinX || pos.x - worldRadius > viewMaxX ||
		    pos.y + worldRadius < viewMinY || pos.y - worldRadius > viewMaxY) {
			continue;
		}

		bool drawAsShip = false;
		float facing = 0.0f;
		if (multiplayerShipFacings != nullptr) {
			const auto it = multiplayerShipFacings->find(body.id);
			if (it != multiplayerShipFacings->end()) {
				drawAsShip = true;
				facing = it->second;
			}
		}
		if (isPlayer && playerFacingRadians.has_value()) {
			drawAsShip = true;
			facing = *playerFacingRadians;
		}

		if (drawAsShip) {
			const float minShipPx = 10.0f;
			const float shipPx = std::max(minShipPx, std::max(radiusPx * 2.0f, 6.0f));
			const float shipWorldRadius = shipPx * worldUnitsPerPixel() * 0.5f;
			const float outlineW = 1.0f * worldUnitsPerPixel();
			if (isPlayer) {
				drawOrientedShip(window_, pos, facing, shipWorldRadius, outlineW,
				                 sf::Color(255, 225, 150, 230), sf::Color(20, 20, 28, 230),
				                 sf::Color(255, 255, 255, 72));
			} else {
				drawOrientedShip(window_, pos, facing, shipWorldRadius, outlineW,
				                 sf::Color(130, 200, 255, 228), sf::Color(18, 40, 58, 235),
				                 sf::Color(200, 235, 255, 85));
			}
		} else if (isShell) {
			const float shellWorldRadiusMin = 1.5f * worldUnitsPerPixel();
			drawShellWithBlast(window_, pos, std::max(worldRadius, shellWorldRadiusMin));
		} else if (radiusPx > 0.5f) {
			circle_.setRadius(worldRadius);
			circle_.setOrigin(sf::Vector2f(worldRadius, worldRadius));
			circle_.setPosition(pos);
			window_.draw(circle_);
		} else {
			points.append(sf::Vertex{pos, sf::Color(210, 230, 255)});
		}
	}

	if (points.getVertexCount() > 0) {
		window_.draw(points);
	}
}

void Renderer::draw(const sim::SimulationEngine& engine) {
	std::vector<sim::BodySnapshot> bodies;
	engine.copyBodies(bodies);
	draw(bodies);
}

}  // namespace render
