#pragma once

#include "ui/InputActions.hpp"
#include "ui/MenuOverlay.hpp"
#include "ui/Predictor.hpp"
#include "ui/SelectionState.hpp"
#include "ui/TraceStore.hpp"

#include <string>

namespace ui {

struct UiState {
	InputActions input;
	SelectionState selection;
	Predictor predictor;
	TraceStore traces;
	MenuOverlay menu;

	enum class FollowCameraMode { FollowOwnShip, FollowSelectionOrOrigin };
	FollowCameraMode followCameraMode = FollowCameraMode::FollowOwnShip;
	bool showShipSelfPrediction = true;
	std::string statusMessage;
};

}  // namespace ui
