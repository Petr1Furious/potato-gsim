#include "ui/Predictor.hpp"

#include <algorithm>
#include <cmath>

namespace ui {

void Predictor::scaleHorizon(double factor) {
	settings_.steps = std::clamp(static_cast<int>(std::lround(settings_.steps * factor)), 20, 2000);
}

std::vector<sim::BodySnapshot> Predictor::trimmedAttractors(
    const std::vector<sim::BodySnapshot>& bodies,
    const sf::Vector2f& around) const {
	if (bodies.size() <= settings_.maxAttractors) {
		return bodies;
	}

	std::vector<sim::BodySnapshot> sorted = bodies;
	std::nth_element(
	    sorted.begin(), sorted.begin() + static_cast<std::ptrdiff_t>(settings_.maxAttractors),
	    sorted.end(), [&](const sim::BodySnapshot& a, const sim::BodySnapshot& b) {
		    const double da =
		        (a.x - around.x) * (a.x - around.x) + (a.y - around.y) * (a.y - around.y);
		    const double db =
		        (b.x - around.x) * (b.x - around.x) + (b.y - around.y) * (b.y - around.y);
		    return da < db;
	    });
	sorted.resize(settings_.maxAttractors);
	return sorted;
}

std::vector<sf::Vector2f> Predictor::integrate(const std::vector<sim::BodySnapshot>& attractors,
                                               sim::BodySnapshot target,
                                               double G,
                                               double epsilon) const {
	std::vector<sf::Vector2f> out;
	if (!settings_.enabled || settings_.steps < 2) {
		return out;
	}
	out.reserve(static_cast<std::size_t>(settings_.steps));
	const double eps2 = epsilon * epsilon;
	const double dt = settings_.dt;

	for (int step = 0; step < settings_.steps; ++step) {
		out.emplace_back(static_cast<float>(target.x), static_cast<float>(target.y));
		double ax = 0.0;
		double ay = 0.0;
		for (const sim::BodySnapshot& b : attractors) {
			if (b.id == target.id) {
				continue;
			}
			const double dx = b.x - target.x;
			const double dy = b.y - target.y;
			const double d2 = dx * dx + dy * dy + eps2;
			const double d = std::sqrt(d2);
			const double invD3 = 1.0 / (d2 * d);
			const double f = G * b.mass * invD3;
			ax += dx * f;
			ay += dy * f;
		}

		target.vx += ax * dt;
		target.vy += ay * dt;
		target.x += target.vx * dt;
		target.y += target.vy * dt;
	}

	return out;
}

std::vector<sf::Vector2f> Predictor::predictForBody(const std::vector<sim::BodySnapshot>& bodies,
                                                    sim::BodyId id,
                                                    double G,
                                                    double epsilon) const {
	auto it = std::find_if(bodies.begin(), bodies.end(),
	                       [id](const sim::BodySnapshot& b) { return b.id == id; });
	if (it == bodies.end()) {
		return {};
	}
	const sf::Vector2f around(static_cast<float>(it->x), static_cast<float>(it->y));
	const std::vector<sim::BodySnapshot> attractors = trimmedAttractors(bodies, around);
	return integrate(attractors, *it, G, epsilon);
}

std::vector<sf::Vector2f> Predictor::predictSpawn(const std::vector<sim::BodySnapshot>& bodies,
                                                  const sim::SpawnCommand& spawn,
                                                  double G,
                                                  double epsilon) const {
	sim::BodySnapshot pseudo{
	    .id = 0,
	    .x = spawn.x,
	    .y = spawn.y,
	    .vx = spawn.vx,
	    .vy = spawn.vy,
	    .mass = spawn.mass,
	    .radius = spawn.radius,
	};
	const sf::Vector2f around(static_cast<float>(spawn.x), static_cast<float>(spawn.y));
	const std::vector<sim::BodySnapshot> attractors = trimmedAttractors(bodies, around);
	return integrate(attractors, pseudo, G, epsilon);
}

}  // namespace ui
