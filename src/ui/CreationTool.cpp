#include "ui/CreationTool.hpp"

#include <algorithm>
#include <cmath>

namespace ui {

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

	if (step_ == Step::Inactive) {
		anchorWorld_ = worldPos;
		cursorWorld_ = worldPos;
		previewRadius_ = 1.0f;
		step_ = Step::SetRadius;
		return std::nullopt;
	}

	updateCursor(worldPos, selectedBody);

	if (step_ == Step::SetRadius) {
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
	(void)selectedBody;
	cursorWorld_ = worldPos;
	if (step_ == Step::SetRadius) {
		const sf::Vector2f d = cursorWorld_ - anchorWorld_;
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
	sf::Vector2f velocityVector = (cursorWorld_ - anchorWorld_) / static_cast<float>(simRate);
	const bool useReferenceFrame =
	    (relativeFrame_ || followReferenceFrame_) && selectedBody.has_value();
	if (useReferenceFrame) {
		velocityVector += sf::Vector2f(static_cast<float>(selectedBody->vx),
		                               static_cast<float>(selectedBody->vy));
	}
	const double radius = std::max(0.05, static_cast<double>(previewRadius_));
	double mass = density_ * volumeFromRadius(radius);
	if (negativeMass_) {
		mass = -mass;
	}

	sim::SpawnCommand out{
	    .x = anchorWorld_.x,
	    .y = anchorWorld_.y,
	    .vx = velocityVector.x,
	    .vy = velocityVector.y,
	    .mass = mass,
	    .radius = radius,
	};
	return out;
}

}  // namespace ui
