#include "sim/QuadTree.hpp"

#include <algorithm>
#include <cmath>
#include <limits>
#include <vector>

namespace sim {

namespace {

[[nodiscard]] double safeHalfSize(double minX, double maxX, double minY, double maxY) {
	const double dx = maxX - minX;
	const double dy = maxY - minY;
	const double extent = std::max(dx, dy);
	return std::max(1.0, extent * 0.5 + 1.0);
}

[[nodiscard]] std::uint64_t spreadBits21(std::uint32_t value) {
	std::uint64_t x = value & 0x1FFFFFu;
	x = (x | (x << 32u)) & 0x1F00000000FFFFull;
	x = (x | (x << 16u)) & 0x1F0000FF0000FFull;
	x = (x | (x << 8u)) & 0x100F00F00F00F00Full;
	x = (x | (x << 4u)) & 0x10C30C30C30C30C3ull;
	x = (x | (x << 2u)) & 0x1249249249249249ull;
	return x;
}

}  // namespace

void BarnesHutTree::build(const std::vector<double>& posX,
                          const std::vector<double>& posY,
                          const std::vector<double>& mass) {
	posX_ = &posX;
	posY_ = &posY;
	mass_ = &mass;
	nodes_.clear();
	leafPayloads_.clear();
	overflowBodies_.clear();
	overflowNext_.clear();
	const std::size_t desiredNodes = std::max<std::size_t>(16, posX.size() * 2);
	if (nodes_.capacity() < desiredNodes) {
		nodes_.reserve(desiredNodes);
	}
	if (leafPayloads_.capacity() < desiredNodes) {
		leafPayloads_.reserve(desiredNodes);
	}
	if (overflowBodies_.capacity() < posX.size()) {
		overflowBodies_.reserve(posX.size());
	}
	if (overflowNext_.capacity() < posX.size()) {
		overflowNext_.reserve(posX.size());
	}

	if (posX.empty()) {
		return;
	}

	double minX = std::numeric_limits<double>::infinity();
	double minY = std::numeric_limits<double>::infinity();
	double maxX = -std::numeric_limits<double>::infinity();
	double maxY = -std::numeric_limits<double>::infinity();

	for (std::size_t i = 0; i < posX.size(); ++i) {
		minX = std::min(minX, posX[i]);
		minY = std::min(minY, posY[i]);
		maxX = std::max(maxX, posX[i]);
		maxY = std::max(maxY, posY[i]);
	}

	const double centerX = (minX + maxX) * 0.5;
	const double centerY = (minY + maxY) * 0.5;
	const double halfSize = safeHalfSize(minX, maxX, minY, maxY);
	const double invExtent = halfSize > 0.0 ? (1.0 / (halfSize * 2.0)) : 0.0;

	const int root = createNode(centerX, centerY, halfSize);
	(void)root;

	mortonKeys_.resize(posX.size());
	insertionOrder_.resize(posX.size());
	for (std::uint32_t i = 0; i < static_cast<std::uint32_t>(posX.size()); ++i) {
		mortonKeys_[i] = mortonKey(posX[i], posY[i], minX, minY, invExtent);
		insertionOrder_[i] = i;
	}
	std::sort(insertionOrder_.begin(), insertionOrder_.end(),
	          [&](std::uint32_t a, std::uint32_t b) {
		          if (mortonKeys_[a] != mortonKeys_[b]) {
			          return mortonKeys_[a] < mortonKeys_[b];
		          }
		          return a < b;
	          });

	for (std::uint32_t bodyIndex : insertionOrder_) {
		insertBody(0, bodyIndex, 0);
	}

	accumulateMass(0);
}

void BarnesHutTree::computeAcceleration(std::size_t bodyIndex,
                                        double bodyX,
                                        double bodyY,
                                        double theta,
                                        double epsilonSquared,
                                        double gravitationalConstant,
                                        double& outAx,
                                        double& outAy,
                                        TraversalStats* stats) const {
	if (stats != nullptr) {
		computeAccelerationImpl<true>(bodyIndex, bodyX, bodyY, theta, epsilonSquared,
		                              gravitationalConstant, outAx, outAy, stats);
	} else {
		computeAccelerationImpl<false>(bodyIndex, bodyX, bodyY, theta, epsilonSquared,
		                               gravitationalConstant, outAx, outAy, nullptr);
	}
}

template <bool CollectStats>
void BarnesHutTree::computeAccelerationImpl(std::size_t bodyIndex,
                                            double bodyX,
                                            double bodyY,
                                            double theta,
                                            double epsilonSquared,
                                            double gravitationalConstant,
                                            double& outAx,
                                            double& outAy,
                                            TraversalStats* stats) const {
	outAx = 0.0;
	outAy = 0.0;
	if constexpr (CollectStats) {
		*stats = TraversalStats{};
	}
	if (nodes_.empty()) {
		return;
	}
	const std::vector<double>& posX = *posX_;
	const std::vector<double>& posY = *posY_;
	const std::vector<double>& mass = *mass_;

	static thread_local std::vector<int> stack;
	stack.clear();
	if (stack.capacity() < 256) {
		stack.reserve(256);
	}
	stack.push_back(0);
	const double theta2 = theta * theta;

	while (!stack.empty()) {
		const int nodeIndex = stack.back();
		stack.pop_back();
		const Node& node = nodes_[nodeIndex];
		if constexpr (CollectStats) {
			++stats->nodeVisits;
		}

		if (node.totalMass <= 0.0) {
			continue;
		}

		const double dx = node.comX - bodyX;
		const double dy = node.comY - bodyY;
		const double dist2 = dx * dx + dy * dy + epsilonSquared;
		if (node.isLeaf()) {
			const LeafPayload* leaf = leafPayloadFor(node);
			if (leaf != nullptr) {
				for (std::uint8_t i = 0; i < leaf->inlineBodyCount; ++i) {
					const std::uint32_t other = leaf->inlineBodies[i];
					if (other == bodyIndex) {
						continue;
					}
					const double odx = posX[other] - bodyX;
					const double ody = posY[other] - bodyY;
					const double od2 = odx * odx + ody * ody + epsilonSquared;
					const double invD = 1.0 / std::sqrt(od2);
					const double invDist3 = invD * invD * invD;
					const double factor = gravitationalConstant * mass[other] * invDist3;
					outAx += odx * factor;
					outAy += ody * factor;
					if constexpr (CollectStats) {
						++stats->directBodyInteractions;
					}
				}
				for (std::int32_t overflowIdx = leaf->overflowHead; overflowIdx >= 0;
				     overflowIdx = overflowNext_[overflowIdx]) {
					const std::uint32_t other = overflowBodies_[overflowIdx];
					if (other == bodyIndex) {
						continue;
					}
					const double odx = posX[other] - bodyX;
					const double ody = posY[other] - bodyY;
					const double od2 = odx * odx + ody * ody + epsilonSquared;
					const double invD = 1.0 / std::sqrt(od2);
					const double invDist3 = invD * invD * invD;
					const double factor = gravitationalConstant * mass[other] * invDist3;
					outAx += odx * factor;
					outAy += ody * factor;
					if constexpr (CollectStats) {
						++stats->directBodyInteractions;
					}
				}
			}
			continue;
		}

		const double size2 = node.halfSize * node.halfSize * 4.0;
		if (size2 < theta2 * dist2) {
			const double invD = 1.0 / std::sqrt(dist2);
			const double invDist3 = invD * invD * invD;
			const double factor = gravitationalConstant * node.totalMass * invDist3;
			outAx += dx * factor;
			outAy += dy * factor;
			if constexpr (CollectStats) {
				++stats->aggregateApproximations;
			}
			continue;
		}

		for (const int child : node.children) {
			if (child >= 0) {
				stack.push_back(child);
			}
		}
	}
}

int BarnesHutTree::createNode(double centerX, double centerY, double halfSize) {
	nodes_.push_back(Node{
	    .centerX = centerX,
	    .centerY = centerY,
	    .halfSize = halfSize,
	});
	return static_cast<int>(nodes_.size() - 1);
}

BarnesHutTree::LeafPayload& BarnesHutTree::ensureLeafPayload(Node& node) {
	if (node.leafPayloadIndex < 0) {
		leafPayloads_.push_back(LeafPayload{});
		node.leafPayloadIndex = static_cast<std::int32_t>(leafPayloads_.size() - 1);
	}
	return leafPayloads_[static_cast<std::size_t>(node.leafPayloadIndex)];
}

const BarnesHutTree::LeafPayload* BarnesHutTree::leafPayloadFor(const Node& node) const {
	if (node.leafPayloadIndex < 0) {
		return nullptr;
	}
	return &leafPayloads_[static_cast<std::size_t>(node.leafPayloadIndex)];
}

std::uint64_t BarnesHutTree::mortonKey(double x,
                                       double y,
                                       double minX,
                                       double minY,
                                       double invExtent) {
	if (!(std::isfinite(x) && std::isfinite(y) && std::isfinite(invExtent)) || invExtent <= 0.0) {
		return 0;
	}
	constexpr std::uint32_t kGridMax = (1u << 21u) - 1u;
	const double tx = std::clamp((x - minX) * invExtent, 0.0, 1.0);
	const double ty = std::clamp((y - minY) * invExtent, 0.0, 1.0);
	const std::uint32_t qx = static_cast<std::uint32_t>(tx * static_cast<double>(kGridMax));
	const std::uint32_t qy = static_cast<std::uint32_t>(ty * static_cast<double>(kGridMax));
	return (spreadBits21(qx) << 1u) | spreadBits21(qy);
}

void BarnesHutTree::appendOverflowBody(Node& node, std::uint32_t bodyIndex) {
	LeafPayload& leaf = ensureLeafPayload(node);
	overflowBodies_.push_back(bodyIndex);
	overflowNext_.push_back(leaf.overflowHead);
	leaf.overflowHead = static_cast<std::int32_t>(overflowBodies_.size() - 1);
}

void BarnesHutTree::splitNode(int nodeIndex) {
	const double centerX = nodes_[nodeIndex].centerX;
	const double centerY = nodes_[nodeIndex].centerY;
	const double quarter = nodes_[nodeIndex].halfSize * 0.5;

	const int nw = createNode(centerX - quarter, centerY - quarter, quarter);
	const int ne = createNode(centerX + quarter, centerY - quarter, quarter);
	const int sw = createNode(centerX - quarter, centerY + quarter, quarter);
	const int se = createNode(centerX + quarter, centerY + quarter, quarter);

	// Re-acquire by index after node vector growth to avoid dangling
	// references.
	nodes_[nodeIndex].children[0] = nw;
	nodes_[nodeIndex].children[1] = ne;
	nodes_[nodeIndex].children[2] = sw;
	nodes_[nodeIndex].children[3] = se;
}

int BarnesHutTree::childQuadrant(const Node& node, double x, double y) const {
	const bool east = x >= node.centerX;
	const bool south = y >= node.centerY;
	if (!east && !south) {
		return 0;
	}
	if (east && !south) {
		return 1;
	}
	if (!east && south) {
		return 2;
	}
	return 3;
}

int BarnesHutTree::childForPoint(const Node& node, double x, double y) const {
	const int quadrant = childQuadrant(node, x, y);
	return node.children[quadrant];
}

void BarnesHutTree::insertBody(int nodeIndex, std::size_t bodyIndex, std::size_t depth) {
	Node& node = nodes_[nodeIndex];
	if (node.isLeaf()) {
		LeafPayload& leaf = ensureLeafPayload(node);
		if (leaf.inlineBodyCount < kLeafBodyCapacity) {
			leaf.inlineBodies[leaf.inlineBodyCount++] = static_cast<std::uint32_t>(bodyIndex);
			return;
		}
		if (depth >= kMaxDepth) {
			appendOverflowBody(node, static_cast<std::uint32_t>(bodyIndex));
			return;
		}

		const std::array<std::uint32_t, kLeafBodyCapacity> existingInline = leaf.inlineBodies;
		const std::uint8_t existingInlineCount = leaf.inlineBodyCount;
		const std::int32_t existingOverflowHead = leaf.overflowHead;
		splitNode(nodeIndex);
		nodes_[nodeIndex].leafPayloadIndex = -1;

		for (std::uint8_t i = 0; i < existingInlineCount; ++i) {
			const std::uint32_t existingBody = existingInline[i];
			const Node& splitNodeRef = nodes_[nodeIndex];
			const int child =
			    childForPoint(splitNodeRef, (*posX_)[existingBody], (*posY_)[existingBody]);
			insertBody(child, existingBody, depth + 1);
		}
		{
			const Node& splitNodeRef = nodes_[nodeIndex];
			const int child = childForPoint(splitNodeRef, (*posX_)[bodyIndex], (*posY_)[bodyIndex]);
			insertBody(child, bodyIndex, depth + 1);
		}
		for (std::int32_t overflowIdx = existingOverflowHead; overflowIdx >= 0;
		     overflowIdx = overflowNext_[overflowIdx]) {
			const std::uint32_t existingBody = overflowBodies_[overflowIdx];
			const Node& splitNodeRef = nodes_[nodeIndex];
			const int child =
			    childForPoint(splitNodeRef, (*posX_)[existingBody], (*posY_)[existingBody]);
			insertBody(child, existingBody, depth + 1);
		}
		return;
	}

	double x = (*posX_)[bodyIndex];
	double y = (*posY_)[bodyIndex];
	if (depth >= kMaxDepth) {
		// If many bodies share near-identical positions, apply a tiny
		// deterministic perturbation to break ties and avoid pathological
		// subdivision.
		const double jitter = static_cast<double>((bodyIndex % 1024u) + 1u) * 1e-9;
		x += jitter;
		y -= jitter;
	}

	const int child = childForPoint(node, x, y);
	insertBody(child, bodyIndex, depth + 1);
}

void BarnesHutTree::accumulateMass(int nodeIndex) {
	Node& node = nodes_[nodeIndex];
	if (node.isLeaf()) {
		const LeafPayload* leaf = leafPayloadFor(node);
		if (leaf == nullptr || (leaf->inlineBodyCount == 0 && leaf->overflowHead < 0)) {
			node.totalMass = 0.0;
			node.comX = node.centerX;
			node.comY = node.centerY;
			return;
		}

		double totalMass = 0.0;
		double weightedX = 0.0;
		double weightedY = 0.0;
		for (std::uint8_t i = 0; i < leaf->inlineBodyCount; ++i) {
			const std::uint32_t body = leaf->inlineBodies[i];
			const double m = (*mass_)[body];
			totalMass += m;
			weightedX += (*posX_)[body] * m;
			weightedY += (*posY_)[body] * m;
		}
		for (std::int32_t overflowIdx = leaf->overflowHead; overflowIdx >= 0;
		     overflowIdx = overflowNext_[overflowIdx]) {
			const std::uint32_t body = overflowBodies_[overflowIdx];
			const double m = (*mass_)[body];
			totalMass += m;
			weightedX += (*posX_)[body] * m;
			weightedY += (*posY_)[body] * m;
		}
		node.totalMass = totalMass;
		if (totalMass > 0.0) {
			node.comX = weightedX / totalMass;
			node.comY = weightedY / totalMass;
		} else {
			node.comX = node.centerX;
			node.comY = node.centerY;
		}
		return;
	}

	double totalMass = 0.0;
	double weightedX = 0.0;
	double weightedY = 0.0;

	for (const int child : node.children) {
		if (child < 0) {
			continue;
		}
		accumulateMass(child);
		const Node& c = nodes_[child];
		totalMass += c.totalMass;
		weightedX += c.comX * c.totalMass;
		weightedY += c.comY * c.totalMass;
	}

	node.totalMass = totalMass;
	if (totalMass > 0.0) {
		node.comX = weightedX / totalMass;
		node.comY = weightedY / totalMass;
	} else {
		node.comX = node.centerX;
		node.comY = node.centerY;
	}
}

template void BarnesHutTree::computeAccelerationImpl<true>(std::size_t bodyIndex,
                                                           double bodyX,
                                                           double bodyY,
                                                           double theta,
                                                           double epsilonSquared,
                                                           double gravitationalConstant,
                                                           double& outAx,
                                                           double& outAy,
                                                           TraversalStats* stats) const;
template void BarnesHutTree::computeAccelerationImpl<false>(std::size_t bodyIndex,
                                                            double bodyX,
                                                            double bodyY,
                                                            double theta,
                                                            double epsilonSquared,
                                                            double gravitationalConstant,
                                                            double& outAx,
                                                            double& outAy,
                                                            TraversalStats* stats) const;

}  // namespace sim
