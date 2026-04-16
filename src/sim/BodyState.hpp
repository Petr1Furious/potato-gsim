#pragma once

#include <cassert>
#include <cstddef>
#include <string>
#include <vector>

namespace sim {

struct BodyState {
	std::vector<double> posX;
	std::vector<double> posY;
	std::vector<double> velX;
	std::vector<double> velY;
	std::vector<double> mass;
	std::vector<double> radius;
	std::vector<std::string> name;

	[[nodiscard]] std::size_t size() const { return posX.size(); }

	[[nodiscard]] bool empty() const { return posX.empty(); }

	void clear() {
		posX.clear();
		posY.clear();
		velX.clear();
		velY.clear();
		mass.clear();
		radius.clear();
		name.clear();
	}

	void reserve(std::size_t n) {
		posX.reserve(n);
		posY.reserve(n);
		velX.reserve(n);
		velY.reserve(n);
		mass.reserve(n);
		radius.reserve(n);
		name.reserve(n);
	}

	void pushBack(double x,
	              double y,
	              double vx,
	              double vy,
	              double m,
	              double r,
	              const std::string& n = {}) {
		posX.push_back(x);
		posY.push_back(y);
		velX.push_back(vx);
		velY.push_back(vy);
		mass.push_back(m);
		radius.push_back(r);
		name.push_back(n);
	}

	void swapRemove(std::size_t index) {
		const std::size_t last = size() - 1;
		if (index != last) {
			posX[index] = posX[last];
			posY[index] = posY[last];
			velX[index] = velX[last];
			velY[index] = velY[last];
			mass[index] = mass[last];
			radius[index] = radius[last];
			name[index] = name[last];
		}
		posX.pop_back();
		posY.pop_back();
		velX.pop_back();
		velY.pop_back();
		mass.pop_back();
		radius.pop_back();
		name.pop_back();
	}

	void validate() const {
		assert(posY.size() == size());
		assert(velX.size() == size());
		assert(velY.size() == size());
		assert(mass.size() == size());
		assert(radius.size() == size());
		assert(name.size() == size());
	}
};

}  // namespace sim
