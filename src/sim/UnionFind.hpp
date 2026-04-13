#pragma once

#include <algorithm>
#include <cstddef>
#include <numeric>
#include <vector>

namespace sim {

class UnionFind {
   public:
	explicit UnionFind(std::size_t n = 0) { reset(n); }

	void reset(std::size_t n) {
		parent_.resize(n);
		rank_.assign(n, 0);
		std::iota(parent_.begin(), parent_.end(), 0);
	}

	[[nodiscard]] std::size_t find(std::size_t x) {
		if (parent_[x] != x) {
			parent_[x] = find(parent_[x]);
		}
		return parent_[x];
	}

	void unite(std::size_t a, std::size_t b) {
		std::size_t rootA = find(a);
		std::size_t rootB = find(b);
		if (rootA == rootB) {
			return;
		}

		if (rank_[rootA] < rank_[rootB]) {
			std::swap(rootA, rootB);
		}
		parent_[rootB] = rootA;
		if (rank_[rootA] == rank_[rootB]) {
			++rank_[rootA];
		}
	}

   private:
	std::vector<std::size_t> parent_;
	std::vector<std::size_t> rank_;
};

}  // namespace sim
