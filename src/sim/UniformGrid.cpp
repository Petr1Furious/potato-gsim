#include "sim/UniformGrid.hpp"

#include <algorithm>
#include <cmath>

namespace sim {

UniformGrid::Cell UniformGrid::cellFor(double x, double y) const {
	return Cell{
	    .x = static_cast<int>(std::floor(x / cellSize_)),
	    .y = static_cast<int>(std::floor(y / cellSize_)),
	};
}

void UniformGrid::build(const std::vector<double>& posX,
                        const std::vector<double>& posY,
                        const std::vector<double>& radius,
                        double cellScale) {
	buckets_.clear();
	if (posX.empty()) {
		return;
	}

	double avgRadius = 0.0;
	for (double r : radius) {
		avgRadius += r;
	}
	avgRadius /= static_cast<double>(radius.size());
	cellSize_ = std::max(1.0, avgRadius * std::max(1.0, cellScale));

	for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(posX.size()); ++i) {
		buckets_[cellFor(posX[i], posY[i])].push_back(i);
	}
}

void UniformGrid::findOverlaps(const std::vector<double>& posX,
                               const std::vector<double>& posY,
                               const std::vector<double>& radius,
                               std::vector<OverlapPair>& outPairs) const {
	outPairs.clear();
	if (buckets_.empty()) {
		return;
	}

	std::vector<Cell> keys;
	keys.reserve(buckets_.size());
	for (const auto& it : buckets_) {
		keys.push_back(it.first);
	}
	std::sort(keys.begin(), keys.end(), [](const Cell& a, const Cell& b) {
		if (a.x != b.x) {
			return a.x < b.x;
		}
		return a.y < b.y;
	});

	constexpr Cell neighbors[] = {
	    Cell{1, 0},
	    Cell{0, 1},
	    Cell{1, 1},
	    Cell{1, -1},
	};

	for (const Cell& key : keys) {
		const auto cellIt = buckets_.find(key);
		if (cellIt == buckets_.end()) {
			continue;
		}
		const std::vector<std::uint32_t>& bodies = cellIt->second;

		// Intra-cell
		for (std::size_t i = 0; i < bodies.size(); ++i) {
			for (std::size_t j = i + 1; j < bodies.size(); ++j) {
				const std::uint32_t a = bodies[i];
				const std::uint32_t b = bodies[j];
				const double dx = posX[b] - posX[a];
				const double dy = posY[b] - posY[a];
				const double rr = radius[a] + radius[b];
				if ((dx * dx + dy * dy) <= rr * rr) {
					outPairs.emplace_back(std::min(a, b), std::max(a, b));
				}
			}
		}

		// Inter-cell (forward neighbor set only for determinism and de-duplication)
		for (const Cell& delta : neighbors) {
			const Cell other{
			    .x = key.x + delta.x,
			    .y = key.y + delta.y,
			};
			const auto otherIt = buckets_.find(other);
			if (otherIt == buckets_.end()) {
				continue;
			}

			const std::vector<std::uint32_t>& otherBodies = otherIt->second;
			for (const std::uint32_t a : bodies) {
				for (const std::uint32_t b : otherBodies) {
					const double dx = posX[b] - posX[a];
					const double dy = posY[b] - posY[a];
					const double rr = radius[a] + radius[b];
					if ((dx * dx + dy * dy) <= rr * rr) {
						outPairs.emplace_back(std::min(a, b), std::max(a, b));
					}
				}
			}
		}
	}

	std::sort(outPairs.begin(), outPairs.end());
	outPairs.erase(std::unique(outPairs.begin(), outPairs.end()), outPairs.end());
}

}  // namespace sim
