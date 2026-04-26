#pragma once

#include "scenario/ScenarioManager.hpp"

#include <cstdint>
#include <filesystem>
#include <string>

namespace io {

struct ClientSettings {
	std::string playerName = "Player";
	std::string mpHost = "localhost";
	std::uint16_t mpPort = 27777;
	bool fullscreen = false;
	bool shipMouseAim = true;
	std::uint32_t windowWidth = 1400;
	std::uint32_t windowHeight = 900;
	/// Last singleplayer preset: empty|solar|binary|spiral|random
	std::string spPreset = "empty";
	scenario::RandomPresetConfig randomCfg{};
};

/// Writable config directory (created on save if possible).
[[nodiscard]] std::filesystem::path clientConfigDirectory();

[[nodiscard]] std::filesystem::path clientSettingsPath();

/// Load from disk. Returns true if settings file existed and was read.
[[nodiscard]] bool loadClientSettings(ClientSettings& out, std::string* errorOut = nullptr);
bool saveClientSettings(const ClientSettings& in, std::string* errorOut = nullptr);

}  // namespace io
