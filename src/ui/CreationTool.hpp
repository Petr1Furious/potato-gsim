#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/System/Vector2.hpp>

#include <numbers>
#include <optional>

namespace ui {

class CreationTool {
   public:
	enum class Step { Inactive, SetRadius, SetVelocity };

	void toggleEnabled();
	void setEnabled(bool enabled);
	[[nodiscard]] bool enabled() const { return enabled_; }
	[[nodiscard]] bool isInProgress() const { return step_ != Step::Inactive; }
	[[nodiscard]] Step step() const { return step_; }

	void setDensity(double density);
	[[nodiscard]] double density() const { return density_; }
	void scaleDensity(double factor);

	void toggleNegativeMass();
	void setNegativeMass(bool enabled);
	[[nodiscard]] bool negativeMass() const { return negativeMass_; }

	void toggleRelativeFrame();
	[[nodiscard]] bool relativeFrame() const { return relativeFrame_; }
	void setFollowReferenceFrame(bool enabled) { followReferenceFrame_ = enabled; }
	[[nodiscard]] bool followReferenceFrame() const { return followReferenceFrame_; }

	void cancel();

	std::optional<sim::SpawnCommand> handleLeftClick(
	    const sf::Vector2f& worldPos,
	    const std::optional<sim::BodySnapshot>& selectedBody,
	    double simSecondsPerRealSecond);

	void updateCursor(const sf::Vector2f& worldPos);
	void updateCursor(const sf::Vector2f& worldPos,
	                  const std::optional<sim::BodySnapshot>& selectedBody);

	[[nodiscard]] sf::Vector2f anchor() const { return anchor_; }
	[[nodiscard]] float previewRadius() const { return previewRadius_; }
	[[nodiscard]] sf::Vector2f cursor() const { return cursor_; }
	[[nodiscard]] std::optional<sim::SpawnCommand> previewSpawn(
	    const std::optional<sim::BodySnapshot>& selectedBody,
	    double simSecondsPerRealSecond) const;

   private:
	static double volumeFromRadius(double radius) {
		return (4.0 / 3.0) * std::numbers::pi * radius * radius * radius;
	}

	bool enabled_ = false;
	Step step_ = Step::Inactive;
	double density_ = 1.0e6;
	bool negativeMass_ = false;
	bool relativeFrame_ = false;
	bool followReferenceFrame_ = false;
	sf::Vector2f anchor_{0.0f, 0.0f};
	float previewRadius_ = 1.0f;
	sf::Vector2f cursor_{0.0f, 0.0f};
};

}  // namespace ui
