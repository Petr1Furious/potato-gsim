#pragma once

#include "sim/SimulationEngine.hpp"

#include <SFML/System/Vector2.hpp>

#include <vector>

namespace ui {

class Predictor {
   public:
	struct Settings {
		bool enabled = true;
		int steps = 220;
		double dt = 1.0 / 120.0;
		double realTimeHorizonSeconds = 5.0;
		std::size_t maxAttractors = 1500;
	};

	[[nodiscard]] const Settings& settings() const { return settings_; }
	void setSettings(Settings settings) { settings_ = settings; }
	void scaleHorizon(double factor);

	std::vector<sf::Vector2f> predictForBody(const std::vector<sim::BodySnapshot>& bodies,
	                                         sim::BodyId id,
	                                         double G,
	                                         double epsilon) const;

	std::vector<sf::Vector2f> predictSpawn(const std::vector<sim::BodySnapshot>& bodies,
	                                       const sim::SpawnCommand& spawn,
	                                       double G,
	                                       double epsilon) const;
	std::vector<sf::Vector2f> predictSpawnRelativeToBody(
	    const std::vector<sim::BodySnapshot>& bodies,
	    const sim::SpawnCommand& spawn,
	    sim::BodyId referenceId,
	    double G,
	    double epsilon) const;

   private:
	[[nodiscard]] std::vector<sim::BodySnapshot> trimmedAttractors(
	    const std::vector<sim::BodySnapshot>& bodies,
	    const sf::Vector2f& around) const;

	std::vector<sf::Vector2f> integrate(const std::vector<sim::BodySnapshot>& attractors,
	                                    sim::BodySnapshot target,
	                                    double G,
	                                    double epsilon) const;

	Settings settings_;
};

}  // namespace ui
