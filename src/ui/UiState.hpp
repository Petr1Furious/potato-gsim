#pragma once

#include "ui/CreationTool.hpp"
#include "ui/InputActions.hpp"
#include "ui/MenuOverlay.hpp"
#include "ui/Predictor.hpp"
#include "ui/SelectionState.hpp"
#include "ui/TraceStore.hpp"

#include <cstddef>
#include <string>
#include <vector>

namespace ui {

struct UiState {
	InputActions input;
	SelectionState selection;
	CreationTool creation;
	Predictor predictor;
	TraceStore traces;
	MenuOverlay menu;

	bool showPredictions = true;
	bool showLabels = true;
	bool showDebugInfo = false;
	bool showNegativeMassWarning = true;

	std::size_t presetIndex = 0;
	std::vector<std::string> presetNames;
	std::string statusMessage;
};

}  // namespace ui
