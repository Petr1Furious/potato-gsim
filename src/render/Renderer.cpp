#include "render/Renderer.hpp"

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
	refreshCameraWorldFromView();
}

void Renderer::onResize(const sf::Vector2u& size) {
	if (size.x == 0 || size.y == 0) {
		return;
	}
	enforceAspectRatio();
}

void Renderer::setViewSize(const sf::Vector2f& size) {
	view_.setSize(size);
	enforceAspectRatio();
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

void Renderer::draw(const std::vector<sim::BodySnapshot>& bodies) {
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
		const float radiusPx = static_cast<float>(body.radius) * pixelsPerWorld;
		const float px = static_cast<float>(body.x - ox);
		const float py = static_cast<float>(body.y - oy);
		const sf::Vector2f pos(px, py);
		const float worldRadius = static_cast<float>(body.radius);
		if (pos.x + worldRadius < viewMinX || pos.x - worldRadius > viewMaxX ||
		    pos.y + worldRadius < viewMinY || pos.y - worldRadius > viewMaxY) {
			continue;
		}
		if (radiusPx > 0.5f) {
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
