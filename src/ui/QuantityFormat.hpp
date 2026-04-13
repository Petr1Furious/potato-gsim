#pragma once

#include <string>

namespace ui {

std::string formatTimeLegacy(double seconds);
std::string formatDistanceLegacy(double meters);
std::string formatSpeedLegacy(double metersPerSecond);

}  // namespace ui
