#include "ui/QuantityFormat.hpp"

#include <array>
#include <cmath>
#include <iomanip>
#include <sstream>
#include <string>

namespace ui {

namespace {

struct UnitStep {
	double value;
	const char* suffix;
};

std::string formatWithUnits(double raw, const std::array<UnitStep, 16>& steps) {
	if (!std::isfinite(raw)) {
		return "n/a";
	}
	const double sign = raw < 0.0 ? -1.0 : 1.0;
	const double absRaw = std::abs(raw);
	const UnitStep* chosen = &steps.front();
	for (const UnitStep& step : steps) {
		if (absRaw >= step.value) {
			chosen = &step;
		} else {
			break;
		}
	}
	std::ostringstream ss;
	ss << std::fixed << std::setprecision(3) << (sign * absRaw / chosen->value) << " "
	   << chosen->suffix;
	return ss.str();
}

}  // namespace

std::string formatTimeLegacy(double seconds) {
	static const std::array<UnitStep, 16> kTimeUnits = {{
	    {1.0e-9, "ns"},
	    {1.0e-6, "us"},
	    {1.0e-3, "ms"},
	    {1.0, "s"},
	    {60.0, "m"},
	    {3600.0, "h"},
	    {86400.0, "d"},
	    {31558149.984, "y"},
	    {31558149.984 * 1.0e3, "Ky"},
	    {31558149.984 * 1.0e6, "My"},
	    {31558149.984 * 1.0e9, "Gy"},
	    {31558149.984 * 1.0e12, "Ty"},
	    {31558149.984 * 1.0e15, "Py"},
	    {31558149.984 * 1.0e18, "Ey"},
	    {31558149.984 * 1.0e21, "Zy"},
	    {31558149.984 * 1.0e24, "Yy"},
	}};
	return formatWithUnits(seconds, kTimeUnits);
}

std::string formatDistanceLegacy(double meters) {
	static const std::array<UnitStep, 16> kDistanceUnits = {{
	    {1.0e-9, "nm"},
	    {1.0e-6, "um"},
	    {1.0e-3, "mm"},
	    {1.0, "m"},
	    {1.0e3, "km"},
	    {1.0e6, "Mm"},
	    {1.0e9, "Gm"},
	    {1.0e12, "Tm"},
	    {2.5902068371240e13, "ld"},
	    {9.4607304725808e15, "ly"},
	    {3.085677581491367e16, "pc"},
	    {3.085677581491367e19, "kpc"},
	    {3.085677581491367e22, "Mpc"},
	    {3.085677581491367e25, "Gpc"},
	    {3.085677581491367e28, "Tpc"},
	    {3.085677581491367e31, "Ppc"},
	}};
	return formatWithUnits(meters, kDistanceUnits);
}

std::string formatSpeedLegacy(double metersPerSecond) {
	return formatDistanceLegacy(metersPerSecond) + "/s";
}

}  // namespace ui
