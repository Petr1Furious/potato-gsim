#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/System/Vector2.hpp>

#include <optional>
#include <vector>

namespace ui {

/// Integrated trajectory; `stoppedOnEncounter` means the last point is first surface contact with
/// another body **strictly more massive** than the tracked body (lighter or equal-mass overlaps
/// do not truncate).
struct PredictionPath {
	std::vector<sf::Vector2f> points;
	bool stoppedOnEncounter = false;
};

class Predictor {
   public:
	struct Settings {
		bool enabled = true;
		int steps = 220;
		double dt = 1.0 / 120.0;
		double realTimeHorizonSeconds = 5.0;
		std::size_t maxAttractors = 320;
		double predictionRecalcIntervalSeconds = 0.0;
	};

	[[nodiscard]] const Settings& settings() const { return settings_; }
	void setSettings(Settings settings) { settings_ = settings; }
	void scaleHorizon(double factor);

	[[nodiscard]] PredictionPath predictForBody(const std::vector<sim::BodySnapshot>& bodies,
	                                            sim::BodyId id,
	                                            double G,
	                                            double epsilon) const;

	[[nodiscard]] PredictionPath predictSpawn(const std::vector<sim::BodySnapshot>& bodies,
	                                          const sim::SpawnCommand& spawn,
	                                          double G,
	                                          double epsilon) const;
	[[nodiscard]] PredictionPath predictSpawnRelativeToBody(
	    const std::vector<sim::BodySnapshot>& bodies,
	    const sim::SpawnCommand& spawn,
	    sim::BodyId referenceId,
	    double G,
	    double epsilon) const;

   private:
	[[nodiscard]] std::vector<sim::BodySnapshot> buildWorkingSet(
	    const std::vector<sim::BodySnapshot>& bodies,
	    const sf::Vector2f& around,
	    const std::vector<sim::BodySnapshot>& injectedBodies,
	    const std::vector<sim::BodyId>& requiredIds,
	    double epsilon) const;

	PredictionPath integrateWorkingSet(std::vector<sim::BodySnapshot> bodies,
	                                   sim::BodyId trackedId,
	                                   std::optional<sim::BodyId> referenceId,
	                                   bool relativeOutput,
	                                   double G,
	                                   double epsilon) const;
	static void mergeOverlaps(std::vector<sim::BodySnapshot>& bodies,
	                          sim::BodyId& trackedId,
	                          std::optional<sim::BodyId>& referenceId);

	Settings settings_;
};

}  // namespace ui
