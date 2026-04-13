#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <vector>

namespace sim {

class BarnesHutTree {
   public:
	void build(const std::vector<double>& posX,
	           const std::vector<double>& posY,
	           const std::vector<double>& mass);

	void computeAcceleration(std::size_t bodyIndex,
	                         double bodyX,
	                         double bodyY,
	                         double theta,
	                         double epsilonSquared,
	                         double gravitationalConstant,
	                         double& outAx,
	                         double& outAy) const;

	[[nodiscard]] bool empty() const { return nodes_.empty(); }

   private:
	struct Node {
		double centerX = 0.0;
		double centerY = 0.0;
		double halfSize = 0.0;

		double totalMass = 0.0;
		double comX = 0.0;
		double comY = 0.0;

		std::int32_t singleBody = -1;
		std::vector<std::uint32_t> overflowBodies;
		std::array<int, 4> children{-1, -1, -1, -1};

		[[nodiscard]] bool isLeaf() const {
			return children[0] < 0 && children[1] < 0 && children[2] < 0 && children[3] < 0;
		}
	};

	int createNode(double centerX, double centerY, double halfSize);
	void splitNode(int nodeIndex);
	void insertBody(int nodeIndex, std::size_t bodyIndex, std::size_t depth);
	void accumulateMass(int nodeIndex);

	[[nodiscard]] int childQuadrant(const Node& node, double x, double y) const;
	[[nodiscard]] int childForPoint(const Node& node, double x, double y) const;

	static constexpr std::size_t kLeafBodyCapacity = 8;
	static constexpr std::size_t kMaxDepth = 48;

	const std::vector<double>* posX_ = nullptr;
	const std::vector<double>* posY_ = nullptr;
	const std::vector<double>* mass_ = nullptr;
	std::vector<Node> nodes_;
};

}  // namespace sim
