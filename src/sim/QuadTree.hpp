#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <vector>

namespace sim {

class BarnesHutTree {
   public:
	struct TraversalStats {
		std::uint32_t nodeVisits = 0;
		std::uint32_t directBodyInteractions = 0;
		std::uint32_t aggregateApproximations = 0;
	};

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
	                         double& outAy,
	                         TraversalStats* stats = nullptr) const;

	[[nodiscard]] bool empty() const { return nodes_.empty(); }
	[[nodiscard]] std::size_t nodeCount() const { return nodes_.size(); }

   private:
	static constexpr std::size_t kLeafBodyCapacity = 8;
	static constexpr std::size_t kMaxDepth = 48;

	struct Node {
		double centerX = 0.0;
		double centerY = 0.0;
		double halfSize = 0.0;

		double totalMass = 0.0;
		double comX = 0.0;
		double comY = 0.0;

		std::int32_t leafPayloadIndex = -1;
		std::array<int, 4> children{-1, -1, -1, -1};

		[[nodiscard]] bool isLeaf() const {
			return children[0] < 0 && children[1] < 0 && children[2] < 0 && children[3] < 0;
		}
	};

	struct LeafPayload {
		std::uint8_t inlineBodyCount = 0;
		std::array<std::uint32_t, kLeafBodyCapacity> inlineBodies{};
		std::int32_t overflowHead = -1;
	};

	int createNode(double centerX, double centerY, double halfSize);
	void splitNode(int nodeIndex);
	void insertBody(int nodeIndex, std::size_t bodyIndex, std::size_t depth);
	void accumulateMass(int nodeIndex);
	void appendOverflowBody(Node& node, std::uint32_t bodyIndex);
	LeafPayload& ensureLeafPayload(Node& node);
	[[nodiscard]] const LeafPayload* leafPayloadFor(const Node& node) const;
	[[nodiscard]] static std::uint64_t mortonKey(double x,
	                                             double y,
	                                             double minX,
	                                             double minY,
	                                             double invExtent);
	template <bool CollectStats>
	void computeAccelerationImpl(std::size_t bodyIndex,
	                             double bodyX,
	                             double bodyY,
	                             double theta,
	                             double epsilonSquared,
	                             double gravitationalConstant,
	                             double& outAx,
	                             double& outAy,
	                             TraversalStats* stats) const;

	[[nodiscard]] int childQuadrant(const Node& node, double x, double y) const;
	[[nodiscard]] int childForPoint(const Node& node, double x, double y) const;

	const std::vector<double>* posX_ = nullptr;
	const std::vector<double>* posY_ = nullptr;
	const std::vector<double>* mass_ = nullptr;
	std::vector<Node> nodes_;
	std::vector<LeafPayload> leafPayloads_;
	std::vector<std::uint32_t> overflowBodies_;
	std::vector<std::int32_t> overflowNext_;
	std::vector<std::uint64_t> mortonKeys_;
	std::vector<std::uint32_t> insertionOrder_;
};

}  // namespace sim
