#pragma once

#include "sim/BodyId.hpp"

#include <cstddef>
#include <cstdint>
#include <optional>
#include <vector>

namespace sim {

class IdIndexMap {
   public:
	void clear() {
		generation_.clear();
		indexBySlot_.clear();
		slotByIndex_.clear();
		freeSlots_.clear();
	}

	[[nodiscard]] std::size_t size() const { return slotByIndex_.size(); }

	[[nodiscard]] BodyId addAtDenseIndex(std::uint32_t denseIndex) {
		std::uint32_t slot = kInvalidIndex;
		if (!freeSlots_.empty()) {
			slot = freeSlots_.back();
			freeSlots_.pop_back();
		} else {
			slot = static_cast<std::uint32_t>(generation_.size());
			generation_.push_back(1);
			indexBySlot_.push_back(kInvalidIndex);
		}

		if (generation_[slot] == 0) {
			generation_[slot] = 1;
		}

		indexBySlot_[slot] = denseIndex;
		slotByIndex_.push_back(slot);
		return makeBodyId(slot, generation_[slot]);
	}

	[[nodiscard]] std::optional<std::uint32_t> denseIndexFor(BodyId id) const {
		const std::uint32_t slot = bodyIdSlot(id);
		if (slot >= generation_.size()) {
			return std::nullopt;
		}
		if (generation_[slot] != bodyIdGeneration(id)) {
			return std::nullopt;
		}
		const std::uint32_t denseIndex = indexBySlot_[slot];
		if (denseIndex == kInvalidIndex) {
			return std::nullopt;
		}
		return denseIndex;
	}

	[[nodiscard]] BodyId idAtDenseIndex(std::uint32_t denseIndex) const {
		const std::uint32_t slot = slotByIndex_[denseIndex];
		return makeBodyId(slot, generation_[slot]);
	}

	void removeDenseIndex(std::uint32_t denseIndex) {
		const std::uint32_t lastDense = static_cast<std::uint32_t>(slotByIndex_.size() - 1);
		const std::uint32_t removedSlot = slotByIndex_[denseIndex];

		indexBySlot_[removedSlot] = kInvalidIndex;
		++generation_[removedSlot];
		if (generation_[removedSlot] == 0) {
			generation_[removedSlot] = 1;
		}
		freeSlots_.push_back(removedSlot);

		if (denseIndex != lastDense) {
			const std::uint32_t movedSlot = slotByIndex_[lastDense];
			slotByIndex_[denseIndex] = movedSlot;
			indexBySlot_[movedSlot] = denseIndex;
		}
		slotByIndex_.pop_back();
	}

   private:
	std::vector<std::uint32_t> generation_;
	std::vector<std::uint32_t> indexBySlot_;
	std::vector<std::uint32_t> slotByIndex_;
	std::vector<std::uint32_t> freeSlots_;
};

}  // namespace sim
