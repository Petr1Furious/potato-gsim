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

std::uint64_t UniformGrid::packCellKey(const Cell& cell) {
	return packCellKey(cell.x, cell.y);
}

std::uint64_t UniformGrid::packCellKey(int x, int y) {
	const std::uint64_t ux = static_cast<std::uint64_t>(static_cast<std::uint32_t>(x));
	const std::uint64_t uy = static_cast<std::uint64_t>(static_cast<std::uint32_t>(y));
	return (ux << 32u) | uy;
}

void UniformGrid::build(const std::vector<double>& posX,
                        const std::vector<double>& posY,
                        const std::vector<double>& radius,
                        double cellScale) {
	entries_.clear();
	ranges_.clear();
	rangeByKey_.clear();
	if (posX.empty()) {
		return;
	}

	double avgRadius = 0.0;
	double maxRadius = 0.0;
	for (double r : radius) {
		avgRadius += r;
		if (r > maxRadius)
			maxRadius = r;
	}
	avgRadius /= static_cast<double>(radius.size());
	cellSize_ = std::max({1.0, avgRadius * std::max(1.0, cellScale), 2.0 * maxRadius});

	entries_.reserve(posX.size());
	for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(posX.size()); ++i) {
		const Cell cell = cellFor(posX[i], posY[i]);
		entries_.push_back(Entry{
		    .key = packCellKey(cell),
		    .bodyIndex = i,
		});
	}

	std::sort(entries_.begin(), entries_.end(), [](const Entry& a, const Entry& b) {
		if (a.key != b.key) {
			return a.key < b.key;
		}
		return a.bodyIndex < b.bodyIndex;
	});

	rangeByKey_.reserve(entries_.size() / 4 + 1);
	std::size_t begin = 0;
	while (begin < entries_.size()) {
		const std::uint64_t key = entries_[begin].key;
		std::size_t end = begin + 1;
		while (end < entries_.size() && entries_[end].key == key) {
			++end;
		}
		const std::uint32_t ux = static_cast<std::uint32_t>(key >> 32u);
		const std::uint32_t uy = static_cast<std::uint32_t>(key & 0xFFFFFFFFu);
		ranges_.push_back(CellRange{
		    .cell =
		        Cell{
		            .x = static_cast<int>(static_cast<std::int32_t>(ux)),
		            .y = static_cast<int>(static_cast<std::int32_t>(uy)),
		        },
		    .begin = begin,
		    .end = end,
		});
		rangeByKey_.emplace(key, ranges_.size() - 1);
		begin = end;
	}
}

void UniformGrid::findOverlaps(const std::vector<double>& posX,
                               const std::vector<double>& posY,
                               const std::vector<double>& radius,
                               std::vector<OverlapPair>& outPairs) const {
	outPairs.clear();
	if (ranges_.empty()) {
		return;
	}
	outPairs.reserve(entries_.size() / 2 + 8);

	constexpr Cell neighbors[] = {
	    Cell{1, 0},
	    Cell{0, 1},
	    Cell{1, 1},
	    Cell{1, -1},
	};

	auto maybePushOverlap = [&](std::uint32_t a, std::uint32_t b) {
		const double dx = posX[b] - posX[a];
		const double dy = posY[b] - posY[a];
		const double rr = radius[a] + radius[b];
		if ((dx * dx + dy * dy) <= rr * rr) {
			outPairs.emplace_back(std::min(a, b), std::max(a, b));
		}
	};

	for (const CellRange& range : ranges_) {
		// Intra-cell
		for (std::size_t i = range.begin; i < range.end; ++i) {
			const std::uint32_t a = entries_[i].bodyIndex;
			for (std::size_t j = i + 1; j < range.end; ++j) {
				const std::uint32_t b = entries_[j].bodyIndex;
				maybePushOverlap(a, b);
			}
		}

		// Inter-cell (forward-neighbor set for deterministic de-duplication).
		for (const Cell& delta : neighbors) {
			const Cell otherCell{
			    .x = range.cell.x + delta.x,
			    .y = range.cell.y + delta.y,
			};
			const auto lookup = rangeByKey_.find(packCellKey(otherCell));
			if (lookup == rangeByKey_.end()) {
				continue;
			}
			const CellRange& otherRange = ranges_[lookup->second];

			for (std::size_t i = range.begin; i < range.end; ++i) {
				const std::uint32_t a = entries_[i].bodyIndex;
				for (std::size_t j = otherRange.begin; j < otherRange.end; ++j) {
					const std::uint32_t b = entries_[j].bodyIndex;
					maybePushOverlap(a, b);
				}
			}
		}
	}
}

}  // namespace sim
