#pragma once

#include <cstdint>

namespace sim {

using BodyId = std::uint64_t;

constexpr std::uint32_t kInvalidIndex = 0xFFFFFFFFu;

[[nodiscard]] constexpr BodyId makeBodyId(std::uint32_t slot, std::uint32_t generation) {
	return (static_cast<BodyId>(generation) << 32u) | static_cast<BodyId>(slot);
}

[[nodiscard]] constexpr std::uint32_t bodyIdSlot(BodyId id) {
	return static_cast<std::uint32_t>(id & 0xFFFFFFFFu);
}

[[nodiscard]] constexpr std::uint32_t bodyIdGeneration(BodyId id) {
	return static_cast<std::uint32_t>(id >> 32u);
}

}  // namespace sim
