#include "io/ClientSettings.hpp"

#include <cstdlib>
#include <cstring>
#include <fstream>

namespace io {
namespace {

std::string trim(std::string s) {
	while (!s.empty() &&
	       (s.back() == ' ' || s.back() == '\t' || s.back() == '\r' || s.back() == '\n')) {
		s.pop_back();
	}
	std::size_t i = 0;
	while (i < s.size() && (s[i] == ' ' || s[i] == '\t')) {
		++i;
	}
	if (i > 0) {
		s.erase(0, i);
	}
	return s;
}

bool parseBool(const std::string& s, bool& out) {
	if (s == "1" || s == "true" || s == "yes" || s == "on") {
		out = true;
		return true;
	}
	if (s == "0" || s == "false" || s == "no" || s == "off") {
		out = false;
		return true;
	}
	return false;
}

}  // namespace

std::filesystem::path clientConfigDirectory() {
#if defined(_WIN32)
	const char* appdata = std::getenv("APPDATA");
	if (appdata != nullptr && appdata[0] != '\0') {
		return std::filesystem::path(appdata) / "potato_gsim";
	}
	const char* home = std::getenv("USERPROFILE");
	if (home != nullptr && home[0] != '\0') {
		return std::filesystem::path(home) / "AppData" / "Roaming" / "potato_gsim";
	}
	return std::filesystem::path("potato_gsim_settings_dir");
#elif defined(__APPLE__)
	const char* home = std::getenv("HOME");
	if (home != nullptr && home[0] != '\0') {
		return std::filesystem::path(home) / "Library" / "Application Support" / "potato_gsim";
	}
	return std::filesystem::path("/tmp/potato_gsim");
#else
	const char* home = std::getenv("HOME");
	if (home != nullptr && home[0] != '\0') {
		return std::filesystem::path(home) / ".config" / "potato_gsim";
	}
	const char* xdg = std::getenv("XDG_CONFIG_HOME");
	if (xdg != nullptr && xdg[0] != '\0') {
		return std::filesystem::path(xdg) / "potato_gsim";
	}
	return std::filesystem::path("/tmp/potato_gsim");
#endif
}

std::filesystem::path clientSettingsPath() {
	return clientConfigDirectory() / "settings.txt";
}

bool loadClientSettings(ClientSettings& out, std::string* errorOut) {
	out = ClientSettings{};
	std::ifstream in(clientSettingsPath());
	if (!in) {
		if (errorOut != nullptr) {
			errorOut->clear();
		}
		return false;
	}
	std::string line;
	while (std::getline(in, line)) {
		line = trim(std::move(line));
		if (line.empty() || line[0] == '#') {
			continue;
		}
		const std::size_t eq = line.find('=');
		if (eq == std::string::npos) {
			continue;
		}
		const std::string key = trim(line.substr(0, eq));
		const std::string val = trim(line.substr(eq + 1));
		if (key == "playerName") {
			out.playerName = val.empty() ? "Player" : val;
		} else if (key == "mpHost") {
			out.mpHost = val.empty() ? "localhost" : val;
		} else if (key == "mpPort") {
			const unsigned long p = std::strtoul(val.c_str(), nullptr, 10);
			if (p > 0 && p <= 65535u) {
				out.mpPort = static_cast<std::uint16_t>(p);
			}
		} else if (key == "fullscreen") {
			(void)parseBool(val, out.fullscreen);
		} else if (key == "shipMouseAim") {
			(void)parseBool(val, out.shipMouseAim);
		} else if (key == "windowWidth") {
			const unsigned long w = std::strtoul(val.c_str(), nullptr, 10);
			if (w >= 320) {
				out.windowWidth = static_cast<std::uint32_t>(w);
			}
		} else if (key == "windowHeight") {
			const unsigned long h = std::strtoul(val.c_str(), nullptr, 10);
			if (h >= 240) {
				out.windowHeight = static_cast<std::uint32_t>(h);
			}
		} else if (key == "spPreset") {
			out.spPreset = val.empty() ? "empty" : val;
		} else if (key == "randomCount") {
			out.randomCfg.count = std::max<std::size_t>(
			    1, static_cast<std::size_t>(std::strtoull(val.c_str(), nullptr, 10)));
		} else if (key == "randomCenterX") {
			out.randomCfg.centerX = std::strtod(val.c_str(), nullptr);
		} else if (key == "randomCenterY") {
			out.randomCfg.centerY = std::strtod(val.c_str(), nullptr);
		} else if (key == "randomSpread") {
			out.randomCfg.spreadRadius = std::max(1.0, std::strtod(val.c_str(), nullptr));
		}
	}
	if (errorOut != nullptr) {
		errorOut->clear();
	}
	return true;
}

bool saveClientSettings(const ClientSettings& in, std::string* errorOut) {
	try {
		const std::filesystem::path dir = clientConfigDirectory();
		std::filesystem::create_directories(dir);
	} catch (const std::exception& e) {
		if (errorOut != nullptr) {
			*errorOut = e.what();
		}
		return false;
	}
	const std::filesystem::path path = clientSettingsPath();
	std::ofstream out(path, std::ios::trunc);
	if (!out) {
		if (errorOut != nullptr) {
			*errorOut = "Could not open settings file for write";
		}
		return false;
	}
	out << "# potato_gsim client settings\n";
	out << "playerName=" << in.playerName << '\n';
	out << "mpHost=" << in.mpHost << '\n';
	out << "mpPort=" << static_cast<unsigned>(in.mpPort) << '\n';
	out << "fullscreen=" << (in.fullscreen ? "1" : "0") << '\n';
	out << "shipMouseAim=" << (in.shipMouseAim ? "1" : "0") << '\n';
	out << "windowWidth=" << in.windowWidth << '\n';
	out << "windowHeight=" << in.windowHeight << '\n';
	out << "spPreset=" << in.spPreset << '\n';
	out << "randomCount=" << in.randomCfg.count << '\n';
	out << "randomCenterX=" << in.randomCfg.centerX << '\n';
	out << "randomCenterY=" << in.randomCfg.centerY << '\n';
	out << "randomSpread=" << in.randomCfg.spreadRadius << '\n';
	if (errorOut != nullptr) {
		errorOut->clear();
	}
	return true;
}

}  // namespace io
