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

}  // namespace

void BarnesHutTree::build(const std::vector<double>& posX,
                          const std::vector<double>& posY,
                          const std::vector<double>& mass) {
	posX_ = &posX;
	posY_ = &posY;
	mass_ = &mass;
	nodes_.clear();
	const std::size_t desiredNodes = std::max<std::size_t>(16, posX.size() * 2);
	if (nodes_.capacity() < desiredNodes) {
		nodes_.reserve(desiredNodes);
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

	const int root = createNode(centerX, centerY, halfSize);
	(void)root;

	for (std::size_t i = 0; i < posX.size(); ++i) {
		insertBody(0, i, 0);
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
                                        double& outAy) const {
	outAx = 0.0;
	outAy = 0.0;
	if (nodes_.empty()) {
		return;
	}

	static thread_local std::vector<int> stack;
	stack.clear();
	if (stack.capacity() < 256) {
		stack.reserve(256);
	}
	stack.push_back(0);

	while (!stack.empty()) {
		const int nodeIndex = stack.back();
		stack.pop_back();
		const Node& node = nodes_[nodeIndex];

		if (node.totalMass <= 0.0) {
			continue;
		}

		const double dx = node.comX - bodyX;
		const double dy = node.comY - bodyY;
		const double dist2 = dx * dx + dy * dy + epsilonSquared;
		const double dist = std::sqrt(dist2);

		if (node.isLeaf()) {
			if (node.singleBody >= 0) {
				const std::uint32_t other = static_cast<std::uint32_t>(node.singleBody);
				if (other != bodyIndex) {
					const double odx = (*posX_)[other] - bodyX;
					const double ody = (*posY_)[other] - bodyY;
					const double od2 = odx * odx + ody * ody + epsilonSquared;
					const double od = std::sqrt(od2);
					const double invDist3 = 1.0 / (od2 * od);
					const double factor = gravitationalConstant * (*mass_)[other] * invDist3;
					outAx += odx * factor;
					outAy += ody * factor;
				}
			}
			for (std::uint32_t other : node.overflowBodies) {
				if (other == bodyIndex) {
					continue;
				}
				const double odx = (*posX_)[other] - bodyX;
				const double ody = (*posY_)[other] - bodyY;
				const double od2 = odx * odx + ody * ody + epsilonSquared;
				const double od = std::sqrt(od2);
				const double invDist3 = 1.0 / (od2 * od);
				const double factor = gravitationalConstant * (*mass_)[other] * invDist3;
				outAx += odx * factor;
				outAy += ody * factor;
			}
			continue;
		}

		const double size = node.halfSize * 2.0;
		if ((size / dist) < theta) {
			const double invDist3 = 1.0 / (dist2 * dist);
			const double factor = gravitationalConstant * node.totalMass * invDist3;
			outAx += dx * factor;
			outAy += dy * factor;
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
	if (nodes_[nodeIndex].isLeaf()) {
		Node& leaf = nodes_[nodeIndex];
		auto appendToLeaf = [&](std::uint32_t idx) {
			if (leaf.singleBody < 0 && leaf.overflowBodies.empty()) {
				leaf.singleBody = static_cast<std::int32_t>(idx);
			} else {
				leaf.overflowBodies.push_back(idx);
			}
		};
		if (leaf.singleBody < 0 && leaf.overflowBodies.empty()) {
			leaf.singleBody = static_cast<std::int32_t>(bodyIndex);
			return;
		}
		const std::size_t leafCount =
		    (leaf.singleBody >= 0 ? 1u : 0u) + leaf.overflowBodies.size();
		if (leafCount < kLeafBodyCapacity || depth >= kMaxDepth) {
			appendToLeaf(static_cast<std::uint32_t>(bodyIndex));
			return;
		}

		std::vector<std::uint32_t> leafBodies;
		leafBodies.reserve(1 + leaf.overflowBodies.size() + 1);
		if (leaf.singleBody >= 0) {
			leafBodies.push_back(static_cast<std::uint32_t>(leaf.singleBody));
		}
		leafBodies.insert(leafBodies.end(), leaf.overflowBodies.begin(), leaf.overflowBodies.end());
		leafBodies.push_back(static_cast<std::uint32_t>(bodyIndex));

		splitNode(nodeIndex);
		nodes_[nodeIndex].singleBody = -1;
		nodes_[nodeIndex].overflowBodies.clear();

		for (const std::uint32_t leafBody : leafBodies) {
			const Node& splitNodeRef = nodes_[nodeIndex];
			const int child = childForPoint(splitNodeRef, (*posX_)[leafBody], (*posY_)[leafBody]);
			insertBody(child, leafBody, depth + 1);
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

	const Node& node = nodes_[nodeIndex];
	const int child = childForPoint(node, x, y);
	insertBody(child, bodyIndex, depth + 1);
}

void BarnesHutTree::accumulateMass(int nodeIndex) {
	Node& node = nodes_[nodeIndex];
	if (node.isLeaf()) {
		if (node.singleBody < 0 && node.overflowBodies.empty()) {
			node.totalMass = 0.0;
			node.comX = node.centerX;
			node.comY = node.centerY;
			return;
		}

		double totalMass = 0.0;
		double weightedX = 0.0;
		double weightedY = 0.0;
		if (node.singleBody >= 0) {
			const std::uint32_t body = static_cast<std::uint32_t>(node.singleBody);
			const double m = (*mass_)[body];
			totalMass += m;
			weightedX += (*posX_)[body] * m;
			weightedY += (*posY_)[body] * m;
		}
		for (const std::uint32_t body : node.overflowBodies) {
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

}  // namespace sim
