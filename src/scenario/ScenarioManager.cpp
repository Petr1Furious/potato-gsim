#include "scenario/ScenarioManager.hpp"

#include <cmath>
#include <cstdint>
#include <numbers>
#include <random>

namespace scenario {

namespace {

constexpr double kG = 6.67430e-11;
constexpr double kEarthLikeDensity = 5514.0;

double radiusFromMass(double mass) {
	return std::cbrt((3.0 * mass) / (4.0 * std::numbers::pi * kEarthLikeDensity));
}

std::vector<sim::SpawnCommand> makeSolarLike() {
	std::vector<sim::SpawnCommand> bodies;
	bodies.reserve(11);
	bodies.push_back(sim::SpawnCommand{
	    .x = 0.0,
	    .y = 0.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 1.98847e30,
	    .radius = 6.9634e8,
	    .name = "Sun",
	});
	const struct Orbit {
		double r;
		double mass;
		double radius;
		double speed;
		const char* name;
	} planets[] = {
	    {5.790905e10, 3.3011e23, 2.4397e6, 4.736e4, "Mercury"},
	    {1.08208e11, 4.8675e24, 6.0518e6, 3.502e4, "Venus"},
	    {1.49598e11, 5.97237e24, 6.371e6, 2.978e4, "Earth"},
	    {2.27939e11, 6.4171e23, 3.3895e6, 2.407e4, "Mars"},
	    {7.7857e11, 1.8982e27, 6.9911e7, 1.307e4, "Jupiter"},
	    {1.43353e12, 5.6834e26, 5.8232e7, 9.68e3, "Saturn"},
	    {2.87246e12, 8.6810e25, 2.5362e7, 6.80e3, "Uranus"},
	    {4.49506e12, 1.02413e26, 2.4622e7, 5.43e3, "Neptune"},
	};
	std::size_t index = 0;
	for (const Orbit& p : planets) {
		bodies.push_back(sim::SpawnCommand{
		    .x = p.r,
		    .y = 0.0,
		    .vx = 0.0,
		    .vy = p.speed,
		    .mass = p.mass,
		    .radius = p.radius,
		    .name = p.name,
		});
		// Moon: mean distance ~384400 km; v = sqrt(G*M_earth / a); velocity added in +y (Sun
		// frame).
		if (index == 2) {
			constexpr double moonSemiMajorAxis = 3.844e8;
			constexpr double moonMass = 7.342e22;
			constexpr double moonRadius = 1.7371e6;
			const double vMoon = std::sqrt(kG * p.mass / moonSemiMajorAxis);
			bodies.push_back(sim::SpawnCommand{
			    .x = p.r + moonSemiMajorAxis,
			    .y = 0.0,
			    .vx = 0.0,
			    .vy = p.speed + vMoon,
			    .mass = moonMass,
			    .radius = moonRadius,
			    .name = "Moon",
			});
		}
		++index;
	}
	return bodies;
}

std::vector<sim::SpawnCommand> makeBinaryDance() {
	std::vector<sim::SpawnCommand> bodies;
	bodies.reserve(260);
	const double starSeparation = 2.8e11;
	const double starMass = 1.15e30;
	const double starRadius = 6.5e8;
	const double orbitRadius = starSeparation * 0.5;
	const double starSpeed = std::sqrt(kG * starMass / (4.0 * orbitRadius));
	bodies.push_back(sim::SpawnCommand{
	    .x = -orbitRadius,
	    .y = 0.0,
	    .vx = 0.0,
	    .vy = -starSpeed,
	    .mass = starMass,
	    .radius = starRadius,
	});
	bodies.push_back(sim::SpawnCommand{
	    .x = orbitRadius,
	    .y = 0.0,
	    .vx = 0.0,
	    .vy = starSpeed,
	    .mass = starMass,
	    .radius = starRadius,
	});

	std::mt19937_64 rng(20260412);
	std::uniform_real_distribution<double> angleDist(0.0, 2.0 * std::numbers::pi);
	std::uniform_real_distribution<double> radiusDist(4.5e11, 2.4e12);
	std::uniform_real_distribution<double> massDist(2e22, 4e25);
	for (int i = 0; i < 256; ++i) {
		const double a = angleDist(rng);
		const double r = radiusDist(rng);
		const double x = std::cos(a) * r;
		const double y = std::sin(a) * r;
		const double tangent = std::sqrt(std::max(0.0, kG * (2.0 * starMass) / std::max(1.0, r)));
		const double mass = massDist(rng);
		bodies.push_back(sim::SpawnCommand{
		    .x = x,
		    .y = y,
		    .vx = -std::sin(a) * tangent,
		    .vy = std::cos(a) * tangent,
		    .mass = mass,
		    .radius = radiusFromMass(mass),
		});
	}
	return bodies;
}

std::vector<sim::SpawnCommand> makeSpiralCluster() {
	std::vector<sim::SpawnCommand> bodies;
	bodies.reserve(1200);
	bodies.push_back(sim::SpawnCommand{
	    .x = 0.0,
	    .y = 0.0,
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 1.98847e30,
	    .radius = 6.9634e8,
	});
	std::mt19937_64 rng(0xC0FFEEu);
	std::uniform_real_distribution<double> armDist(0.0, 1.0);
	std::uniform_real_distribution<double> jitter(-2.0e9, 2.0e9);
	std::uniform_real_distribution<double> massDist(5e21, 2e25);
	for (int i = 0; i < 1200; ++i) {
		const double arm = armDist(rng) < 0.5 ? 0.0 : std::numbers::pi;
		const double t = static_cast<double>(i) / 45.0;
		const double r = 2.5e9 * t + 4.5e10;
		const double a = arm + t * 0.75;
		const double x = std::cos(a) * r + jitter(rng);
		const double y = std::sin(a) * r + jitter(rng);
		const double tangent = std::sqrt(std::max(0.0, kG * 1.98847e30 / std::max(1.0, r)));
		const double mass = massDist(rng);
		bodies.push_back(sim::SpawnCommand{
		    .x = x,
		    .y = y,
		    .vx = -std::sin(a) * tangent,
		    .vy = std::cos(a) * tangent,
		    .mass = mass,
		    .radius = radiusFromMass(mass),
		});
	}
	return bodies;
}

}  // namespace

std::vector<std::string> ScenarioManager::presetNames() {
	return {
	    "Solar-Like",
	    "Binary Dance",
	    "Spiral Cluster",
	};
}

std::vector<sim::SpawnCommand> ScenarioManager::makePreset(std::size_t index) {
	switch (index % 3) {
		case 0:
			return makeSolarLike();
		case 1:
			return makeBinaryDance();
		default:
			return makeSpiralCluster();
	}
}

std::vector<sim::SpawnCommand> ScenarioManager::makeRandom(std::size_t count,
                                                           double centerX,
                                                           double centerY,
                                                           double spreadRadius) {
	std::vector<sim::SpawnCommand> bodies;
	bodies.reserve(count);
	std::random_device rd;
	std::mt19937_64 rng((static_cast<std::uint64_t>(rd()) << 1u) ^ 0x9E3779B97F4A7C15ULL);
	std::uniform_real_distribution<double> angleDist(0.0, 2.0 * std::numbers::pi);
	std::uniform_real_distribution<double> radialDist(0.0, 1.0);
	std::uniform_real_distribution<double> massDist(1e21, 5e25);
	std::uniform_real_distribution<double> jitter(-1.5e9, 1.5e9);

	for (std::size_t i = 0; i < count; ++i) {
		const double a = angleDist(rng);
		const double r = std::sqrt(radialDist(rng)) * spreadRadius;
		const double x = centerX + std::cos(a) * r + jitter(rng);
		const double y = centerY + std::sin(a) * r + jitter(rng);
		const double mass = massDist(rng);
		bodies.push_back(sim::SpawnCommand{
		    .x = x,
		    .y = y,
		    .vx = 0,
		    .vy = 0,
		    .mass = mass,
		    .radius = radiusFromMass(mass),
		});
	}
	return bodies;
}

}  // namespace scenario
