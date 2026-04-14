#include "ui/CreationTool.hpp"

#include <algorithm>
#include <cmath>

namespace ui {

namespace {

sf::Vector2f toLocalFrame(const sf::Vector2f& worldPos,
                          bool useReferenceFrame,
                          const std::optional<sim::BodySnapshot>& selectedBody) {
	if (useReferenceFrame && selectedBody.has_value()) {
		return sf::Vector2f(worldPos.x - static_cast<float>(selectedBody->x),
		                    worldPos.y - static_cast<float>(selectedBody->y));
	}
	return worldPos;
}

}  // namespace

void CreationTool::toggleEnabled() {
	enabled_ = !enabled_;
	if (!enabled_) {
		cancel();
	}
}

void CreationTool::setEnabled(bool enabled) {
	enabled_ = enabled;
	if (!enabled_) {
		cancel();
	}
}

void CreationTool::setDensity(double density) {
	density_ = std::clamp(density, 100.0, 30000.0);
}

void CreationTool::scaleDensity(double factor) {
	setDensity(density_ * factor);
}

void CreationTool::toggleNegativeMass() {
	negativeMass_ = !negativeMass_;
}

void CreationTool::setNegativeMass(bool enabled) {
	negativeMass_ = enabled;
}

void CreationTool::toggleRelativeFrame() {
	relativeFrame_ = !relativeFrame_;
}

void CreationTool::cancel() {
	step_ = Step::Inactive;
}

std::optional<sim::SpawnCommand> CreationTool::handleLeftClick(
    const sf::Vector2f& worldPos,
    const std::optional<sim::BodySnapshot>& selectedBody,
    double simSecondsPerRealSecond) {
	if (!enabled_) {
		return std::nullopt;
	}

	const bool useReferenceFrame =
	    (relativeFrame_ || followReferenceFrame_) && selectedBody.has_value();
	const sf::Vector2f local = toLocalFrame(worldPos, useReferenceFrame, selectedBody);
	cursor_ = local;
	if (step_ == Step::Inactive) {
		anchor_ = local;
		previewRadius_ = 1.0f;
		step_ = Step::SetRadius;
		return std::nullopt;
	}

	if (step_ == Step::SetRadius) {
		const sf::Vector2f d = local - anchor_;
		previewRadius_ = std::max(0.05f, std::sqrt(d.x * d.x + d.y * d.y));
		step_ = Step::SetVelocity;
		return std::nullopt;
	}

	const std::optional<sim::SpawnCommand> preview =
	    previewSpawn(selectedBody, simSecondsPerRealSecond);
	step_ = Step::Inactive;
	return preview;
}

void CreationTool::updateCursor(const sf::Vector2f& worldPos) {
	updateCursor(worldPos, std::nullopt);
}

void CreationTool::updateCursor(const sf::Vector2f& worldPos,
                                const std::optional<sim::BodySnapshot>& selectedBody) {
	const bool useReferenceFrame =
	    (relativeFrame_ || followReferenceFrame_) && selectedBody.has_value();
	cursor_ = toLocalFrame(worldPos, useReferenceFrame, selectedBody);
	if (step_ == Step::SetRadius) {
		const sf::Vector2f d = cursor_ - anchor_;
		previewRadius_ = std::max(0.05f, std::sqrt(d.x * d.x + d.y * d.y));
	}
}

std::optional<sim::SpawnCommand> CreationTool::previewSpawn(
    const std::optional<sim::BodySnapshot>& selectedBody,
    double simSecondsPerRealSecond) const {
	if (step_ != Step::SetVelocity && step_ != Step::Inactive) {
		return std::nullopt;
	}

	// Creation drag encodes velocity per real second; convert to simulation velocity units.
	const double simRate = (std::isfinite(simSecondsPerRealSecond) && simSecondsPerRealSecond > 0.0)
	                           ? simSecondsPerRealSecond
	                           : 1.0;
	const sf::Vector2f velocityVector = (cursor_ - anchor_) / static_cast<float>(simRate);
	const double radius = std::max(0.05, static_cast<double>(previewRadius_));
	double mass = density_ * volumeFromRadius(radius);
	if (negativeMass_) {
		mass = -mass;
	}

	sim::SpawnCommand out{
	    .x = anchor_.x,
	    .y = anchor_.y,
	    .vx = velocityVector.x,
	    .vy = velocityVector.y,
	    .mass = mass,
	    .radius = radius,
	};
	const bool useReferenceFrame =
	    (relativeFrame_ || followReferenceFrame_) && selectedBody.has_value();
	if (useReferenceFrame) {
		out.x += selectedBody->x;
		out.y += selectedBody->y;
		out.vx += selectedBody->vx;
		out.vy += selectedBody->vy;
	}
	return out;
}

}  // namespace ui
