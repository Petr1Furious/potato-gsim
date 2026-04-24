#include "ui/Predictor.hpp"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <unordered_set>

#include "sim/UnionFind.hpp"

namespace ui {

namespace {

std::optional<std::size_t> findIndexById(const std::vector<sim::BodySnapshot>& bodies,
                                         sim::BodyId id) {
	for (std::size_t i = 0; i < bodies.size(); ++i) {
		if (bodies[i].id == id) {
			return i;
		}
	}
	return std::nullopt;
}

sim::BodyId nextSyntheticId(const std::vector<sim::BodySnapshot>& bodies) {
	sim::BodyId maxId = 0;
	for (const sim::BodySnapshot& body : bodies) {
		maxId = std::max(maxId, body.id);
	}
	return maxId + 1;
}

bool circlesOverlap(const sim::BodySnapshot& a, const sim::BodySnapshot& b) {
	const double dx = b.x - a.x;
	const double dy = b.y - a.y;
	const double rr = a.radius + b.radius;
	return (dx * dx + dy * dy) <= rr * rr;
}

bool trackedOverlapsAnyOther(const std::vector<sim::BodySnapshot>& bodies,
                             std::size_t trackedIndex) {
	const sim::BodySnapshot& t = bodies[trackedIndex];
	for (std::size_t j = 0; j < bodies.size(); ++j) {
		if (j == trackedIndex) {
			continue;
		}
		if (circlesOverlap(t, bodies[j])) {
			return true;
		}
	}
	return false;
}

/// True when the tracked body overlaps another and is **strictly lighter** (by |mass|) than that
/// body — the case where we truncate the prediction path at contact.
bool trackedOverlapsStrictlyMoreMassiveOther(const std::vector<sim::BodySnapshot>& bodies,
                                             std::size_t trackedIndex) {
	const sim::BodySnapshot& t = bodies[trackedIndex];
	const double tMassAbs = std::abs(t.mass);
	for (std::size_t j = 0; j < bodies.size(); ++j) {
		if (j == trackedIndex) {
			continue;
		}
		if (!circlesOverlap(t, bodies[j])) {
			continue;
		}
		if (tMassAbs < std::abs(bodies[j].mass)) {
			return true;
		}
	}
	return false;
}

bool initialTrackedOverlapOthers(const std::vector<sim::BodySnapshot>& bodies,
                                 sim::BodyId trackedId) {
	const std::optional<std::size_t> ti = findIndexById(bodies, trackedId);
	if (!ti.has_value()) {
		return false;
	}
	return trackedOverlapsAnyOther(bodies, *ti);
}

}  // namespace

void Predictor::scaleHorizon(double factor) {
	settings_.steps = std::clamp(static_cast<int>(std::lround(settings_.steps * factor)), 20, 2000);
}

std::vector<sim::BodySnapshot> Predictor::buildWorkingSet(
    const std::vector<sim::BodySnapshot>& bodies,
    const sf::Vector2f& around,
    const std::vector<sim::BodySnapshot>& injectedBodies,
    const std::vector<sim::BodyId>& requiredIds,
    double epsilon) const {
	std::vector<sim::BodySnapshot> selected;
	std::unordered_set<sim::BodyId> selectedIds;
	selectedIds.reserve(injectedBodies.size() + requiredIds.size() + 8);

	std::size_t cap = std::max<std::size_t>(1, settings_.maxAttractors);
	cap = std::max(cap, injectedBodies.size() + requiredIds.size());

	auto pushIfUnique = [&](const sim::BodySnapshot& body) {
		if (selected.size() >= cap) {
			return;
		}
		if (selectedIds.insert(body.id).second) {
			selected.push_back(body);
		}
	};

	for (const sim::BodySnapshot& body : injectedBodies) {
		pushIfUnique(body);
	}
	for (sim::BodyId requiredId : requiredIds) {
		const auto it = std::find_if(
		    bodies.begin(), bodies.end(),
		    [requiredId](const sim::BodySnapshot& body) { return body.id == requiredId; });
		if (it != bodies.end()) {
			pushIfUnique(*it);
		}
	}
	if (selected.size() >= cap) {
		return selected;
	}

	struct Candidate {
		double score = 0.0;
		std::size_t index = 0;
	};
	std::vector<Candidate> candidates;
	candidates.reserve(bodies.size());
	const double eps2 = epsilon * epsilon;
	for (std::size_t i = 0; i < bodies.size(); ++i) {
		const sim::BodySnapshot& body = bodies[i];
		if (selectedIds.contains(body.id)) {
			continue;
		}
		const double dx = body.x - around.x;
		const double dy = body.y - around.y;
		const double influence = std::abs(body.mass) / (dx * dx + dy * dy + eps2);
		candidates.push_back(Candidate{
		    .score = std::isfinite(influence) ? influence : 0.0,
		    .index = i,
		});
	}
	std::sort(candidates.begin(), candidates.end(),
	          [&](const Candidate& a, const Candidate& b) { return a.score > b.score; });
	const std::size_t remaining = cap - selected.size();
	const std::size_t takeCount = std::min(remaining, candidates.size());
	for (std::size_t i = 0; i < takeCount; ++i) {
		pushIfUnique(bodies[candidates[i].index]);
	}
	return selected;
}

void Predictor::mergeOverlaps(std::vector<sim::BodySnapshot>& bodies,
                              sim::BodyId& trackedId,
                              std::optional<sim::BodyId>& referenceId) {
	if (bodies.size() < 2) {
		return;
	}

	sim::UnionFind components(bodies.size());
	bool anyOverlap = false;
	for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(bodies.size()); ++i) {
		for (std::uint32_t j = i + 1; j < static_cast<std::uint32_t>(bodies.size()); ++j) {
			const double dx = bodies[j].x - bodies[i].x;
			const double dy = bodies[j].y - bodies[i].y;
			const double rr = bodies[i].radius + bodies[j].radius;
			if ((dx * dx + dy * dy) <= rr * rr) {
				components.unite(i, j);
				anyOverlap = true;
			}
		}
	}
	if (!anyOverlap) {
		return;
	}

	std::vector<std::vector<std::size_t>> groups(bodies.size());
	for (std::size_t i = 0; i < bodies.size(); ++i) {
		groups[components.find(i)].push_back(i);
	}

	std::vector<sim::BodySnapshot> mergedBodies;
	mergedBodies.reserve(bodies.size());
	for (const std::vector<std::size_t>& group : groups) {
		if (group.empty()) {
			continue;
		}
		if (group.size() == 1) {
			mergedBodies.push_back(bodies[group.front()]);
			continue;
		}

		std::size_t survivor = group.front();
		double survivorAbsMass = std::abs(bodies[survivor].mass);
		for (std::size_t idx : group) {
			const double massAbs = std::abs(bodies[idx].mass);
			const bool betterMass = massAbs > survivorAbsMass;
			const bool tie = std::abs(massAbs - survivorAbsMass) <= 1e-12;
			const bool betterTieBreak = tie && (bodies[idx].id < bodies[survivor].id);
			if (betterMass || betterTieBreak) {
				survivor = idx;
				survivorAbsMass = massAbs;
			}
		}

		const sim::BodyId survivorId = bodies[survivor].id;
		double totalMass = 0.0;
		double momentumX = 0.0;
		double momentumY = 0.0;
		double weightedX = 0.0;
		double weightedY = 0.0;
		double volumeTerm = 0.0;
		bool trackedMerged = false;
		bool referenceMerged = false;
		for (std::size_t idx : group) {
			const sim::BodySnapshot& body = bodies[idx];
			if (body.id == trackedId) {
				trackedMerged = true;
			}
			if (referenceId.has_value() && body.id == *referenceId) {
				referenceMerged = true;
			}
			totalMass += body.mass;
			momentumX += body.vx * body.mass;
			momentumY += body.vy * body.mass;
			weightedX += body.x * body.mass;
			weightedY += body.y * body.mass;
			volumeTerm += body.radius * body.radius * body.radius;
		}
		if (trackedMerged) {
			trackedId = survivorId;
		}
		if (referenceMerged) {
			referenceId = survivorId;
		}

		if (std::abs(totalMass) <= 1e-12) {
			for (std::size_t idx : group) {
				mergedBodies.push_back(bodies[idx]);
			}
			continue;
		}

		sim::BodySnapshot merged = bodies[survivor];
		merged.mass = totalMass;
		merged.vx = momentumX / totalMass;
		merged.vy = momentumY / totalMass;
		merged.x = weightedX / totalMass;
		merged.y = weightedY / totalMass;
		merged.radius = std::cbrt(std::max(0.0, volumeTerm));
		mergedBodies.push_back(std::move(merged));
	}

	bodies.swap(mergedBodies);
}

PredictionPath Predictor::integrateWorkingSet(std::vector<sim::BodySnapshot> bodies,
                                              sim::BodyId trackedId,
                                              std::optional<sim::BodyId> referenceId,
                                              bool relativeOutput,
                                              double G,
                                              double epsilon) const {
	PredictionPath result;
	std::vector<sf::Vector2f>& out = result.points;
	if (!settings_.enabled || settings_.steps < 2 || bodies.empty()) {
		return result;
	}

	const bool initialOverlap = initialTrackedOverlapOthers(bodies, trackedId);
	const double dt = std::max(1e-9, settings_.dt);
	const double eps2 = epsilon * epsilon;
	out.reserve(static_cast<std::size_t>(settings_.steps));

	for (int step = 0; step < settings_.steps; ++step) {
		mergeOverlaps(bodies, trackedId, referenceId);
		const std::optional<std::size_t> trackedIndex = findIndexById(bodies, trackedId);
		if (!trackedIndex.has_value()) {
			break;
		}
		if (relativeOutput) {
			if (!referenceId.has_value()) {
				break;
			}
			const std::optional<std::size_t> referenceIndex = findIndexById(bodies, *referenceId);
			if (!referenceIndex.has_value()) {
				break;
			}
			out.emplace_back(
			    static_cast<float>(bodies[*trackedIndex].x - bodies[*referenceIndex].x),
			    static_cast<float>(bodies[*trackedIndex].y - bodies[*referenceIndex].y));
		} else {
			out.emplace_back(static_cast<float>(bodies[*trackedIndex].x),
			                 static_cast<float>(bodies[*trackedIndex].y));
		}

		std::vector<double> accX(bodies.size(), 0.0);
		std::vector<double> accY(bodies.size(), 0.0);
		for (std::size_t i = 0; i < bodies.size(); ++i) {
			for (std::size_t j = i + 1; j < bodies.size(); ++j) {
				const double dx = bodies[j].x - bodies[i].x;
				const double dy = bodies[j].y - bodies[i].y;
				const double d2 = dx * dx + dy * dy + eps2;
				const double invD = 1.0 / std::sqrt(d2);
				const double invD3 = invD * invD * invD;
				const double f = G * invD3;
				accX[i] += dx * f * bodies[j].mass;
				accY[i] += dy * f * bodies[j].mass;
				accX[j] -= dx * f * bodies[i].mass;
				accY[j] -= dy * f * bodies[i].mass;
			}
		}

		for (std::size_t i = 0; i < bodies.size(); ++i) {
			bodies[i].vx += accX[i] * dt;
			bodies[i].vy += accY[i] * dt;
			bodies[i].x += bodies[i].vx * dt;
			bodies[i].y += bodies[i].vy * dt;
		}

		if (!initialOverlap) {
			const std::optional<std::size_t> postIndex = findIndexById(bodies, trackedId);
			if (!postIndex.has_value()) {
				break;
			}
			if (trackedOverlapsStrictlyMoreMassiveOther(bodies, *postIndex)) {
				const sim::BodySnapshot& t = bodies[*postIndex];
				if (relativeOutput) {
					if (!referenceId.has_value()) {
						break;
					}
					const std::optional<std::size_t> refPost = findIndexById(bodies, *referenceId);
					if (!refPost.has_value()) {
						break;
					}
					const sim::BodySnapshot& r = bodies[*refPost];
					out.emplace_back(static_cast<float>(t.x - r.x), static_cast<float>(t.y - r.y));
				} else {
					out.emplace_back(static_cast<float>(t.x), static_cast<float>(t.y));
				}
				result.stoppedOnEncounter = true;
				break;
			}
		}
	}
	return result;
}

PredictionPath Predictor::predictForBody(const std::vector<sim::BodySnapshot>& bodies,
                                         sim::BodyId id,
                                         double G,
                                         double epsilon) const {
	const auto it = std::find_if(bodies.begin(), bodies.end(),
	                             [id](const sim::BodySnapshot& b) { return b.id == id; });
	if (it == bodies.end()) {
		return {};
	}
	const sf::Vector2f around(static_cast<float>(it->x), static_cast<float>(it->y));
	const std::vector<sim::BodySnapshot> workingSet =
	    buildWorkingSet(bodies, around, {}, {id}, epsilon);
	return integrateWorkingSet(workingSet, id, std::nullopt, false, G, epsilon);
}

PredictionPath Predictor::predictSpawn(const std::vector<sim::BodySnapshot>& bodies,
                                       const sim::SpawnCommand& spawn,
                                       double G,
                                       double epsilon) const {
	const sim::BodyId pseudoId = nextSyntheticId(bodies);
	const sim::BodySnapshot pseudo{
	    .id = pseudoId,
	    .x = spawn.x,
	    .y = spawn.y,
	    .vx = spawn.vx,
	    .vy = spawn.vy,
	    .mass = spawn.mass,
	    .radius = spawn.radius,
	};
	const sf::Vector2f around(static_cast<float>(spawn.x), static_cast<float>(spawn.y));
	std::vector<sim::BodySnapshot> workingSet =
	    buildWorkingSet(bodies, around, {pseudo}, {}, epsilon);
	return integrateWorkingSet(std::move(workingSet), pseudoId, std::nullopt, false, G, epsilon);
}

PredictionPath Predictor::predictSpawnRelativeToBody(const std::vector<sim::BodySnapshot>& bodies,
                                                     const sim::SpawnCommand& spawn,
                                                     sim::BodyId referenceId,
                                                     double G,
                                                     double epsilon) const {
	if (!settings_.enabled || settings_.steps < 2) {
		return {};
	}

	const auto refIt =
	    std::find_if(bodies.begin(), bodies.end(),
	                 [referenceId](const sim::BodySnapshot& b) { return b.id == referenceId; });
	if (refIt == bodies.end()) {
		return {};
	}

	const sim::BodyId pseudoId = nextSyntheticId(bodies);
	const sim::BodySnapshot pseudo{
	    .id = pseudoId,
	    .x = spawn.x,
	    .y = spawn.y,
	    .vx = spawn.vx,
	    .vy = spawn.vy,
	    .mass = spawn.mass,
	    .radius = spawn.radius,
	};
	const sf::Vector2f around(static_cast<float>(spawn.x), static_cast<float>(spawn.y));
	std::vector<sim::BodySnapshot> workingSet =
	    buildWorkingSet(bodies, around, {pseudo}, {referenceId}, epsilon);
	return integrateWorkingSet(std::move(workingSet), pseudoId,
	                           std::optional<sim::BodyId>(referenceId), true, G, epsilon);
}

}  // namespace ui
