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

	struct CellHash {
		[[nodiscard]] std::size_t operator()(const Cell& cell) const {
			const std::uint64_t ux = static_cast<std::uint64_t>(static_cast<std::uint32_t>(cell.x));
			const std::uint64_t uy = static_cast<std::uint64_t>(static_cast<std::uint32_t>(cell.y));
			return static_cast<std::size_t>((ux * 0x9E3779B185EBCA87ULL) ^
			                                (uy * 0xC2B2AE3D27D4EB4FULL));
		}
	};

	[[nodiscard]] Cell cellFor(double x, double y) const;

	double cellSize_ = 1.0;
	std::unordered_map<Cell, std::vector<std::uint32_t>, CellHash> buckets_;
};

}  // namespace sim
