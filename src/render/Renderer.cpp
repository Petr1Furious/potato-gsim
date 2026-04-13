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
	circle_.setPointCount(12);
	enforceAspectRatio();
}

void Renderer::resetView() {
	view_ = window_.getDefaultView();
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
}

void Renderer::panByPixels(const sf::Vector2i& pixelDelta) {
	const float scaleX = view_.getSize().x / static_cast<float>(window_.getSize().x);
	const float scaleY = view_.getSize().y / static_cast<float>(window_.getSize().y);
	view_.move(sf::Vector2f(-pixelDelta.x * scaleX, -pixelDelta.y * scaleY));
}

sf::Vector2f Renderer::screenToWorld(const sf::Vector2i& pixel) const {
	return window_.mapPixelToCoords(pixel, view_);
}

sf::Vector2i Renderer::worldToPixel(const sf::Vector2f& world) const {
	return window_.mapCoordsToPixel(world, view_);
}

float Renderer::worldUnitsPerPixel() const {
	return view_.getSize().x / std::max(1.0f, static_cast<float>(window_.getSize().x));
}

void Renderer::draw(const sim::SimulationEngine& engine) {
	window_.setView(view_);
	window_.clear(sf::Color(8, 10, 16));

	const float pixelsPerWorld = 1.0f / worldUnitsPerPixel();
	sf::VertexArray points(sf::PrimitiveType::Points);

	engine.withReadState([&](const sim::BodyState& state) {
		for (std::size_t i = 0; i < state.size(); ++i) {
			const float radiusPx = static_cast<float>(state.radius[i]) * pixelsPerWorld;
			const sf::Vector2f pos(static_cast<float>(state.posX[i]),
			                       static_cast<float>(state.posY[i]));
			if (radiusPx > 1.0f) {
				const float worldRadius = static_cast<float>(state.radius[i]);
				circle_.setRadius(worldRadius);
				circle_.setOrigin(sf::Vector2f(worldRadius, worldRadius));
				circle_.setPosition(pos);
				window_.draw(circle_);
			} else {
				points.append(sf::Vertex{pos, sf::Color(210, 230, 255)});
			}
		}
	});

	if (points.getVertexCount() > 0) {
		window_.draw(points);
	}
}

}  // namespace render
