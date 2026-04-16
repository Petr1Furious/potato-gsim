#pragma once

#include <cstddef>
#include <cstdint>
#include <unordered_map>
#include <utility>
#include <vector>

namespace sim {

class UniformGrid {
   public:
	using OverlapPair = std::pair<std::uint32_t, std::uint32_t>;

	void build(const std::vector<double>& posX,
	           const std::vector<double>& posY,
	           const std::vector<double>& radius,
	           double cellScale);

	void findOverlaps(const std::vector<double>& posX,
	                  const std::vector<double>& posY,
	                  const std::vector<double>& radius,
	                  std::vector<OverlapPair>& outPairs) const;

   private:
	struct Cell {
		int x = 0;
		int y = 0;

		[[nodiscard]] bool operator==(const Cell& other) const {
			return x == other.x && y == other.y;
		}
	};

	struct Entry {
		std::uint64_t key = 0;
		std::uint32_t bodyIndex = 0;
	};

	struct CellRange {
		Cell cell{};
		std::size_t begin = 0;
		std::size_t end = 0;
	};

	[[nodiscard]] Cell cellFor(double x, double y) const;
	[[nodiscard]] static std::uint64_t packCellKey(const Cell& cell);
	[[nodiscard]] static std::uint64_t packCellKey(int x, int y);

	double cellSize_ = 1.0;
	std::vector<Entry> entries_;
	std::vector<CellRange> ranges_;
	std::unordered_map<std::uint64_t, std::size_t> rangeByKey_;
};

}  // namespace sim
