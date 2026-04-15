#include "ui/CreationPauseStateMachine.hpp"

namespace ui {

bool CreationPauseStateMachine::onCreationClickStart(const sf::Vector2i& clickPixel,
                                                     bool creationEnabled,
                                                     bool creationInProgress,
                                                     sim::SimulationEngine& engine) {
	if (!creationEnabled) {
		return false;
	}
	if (!lockActive_ && !creationInProgress) {
		pausedBeforeCreation_ = engine.config().paused;
		lockActive_ = true;
		awaitingStart_ = true;
		pendingStartPixel_ = clickPixel;
		pauseAckToken_ = engine.requestPauseAck(true);
		engine.setPaused(true);
		return true;
	}
	if (lockActive_ && awaitingStart_) {
		return true;
	}
	return false;
}

bool CreationPauseStateMachine::onEscape(sim::SimulationEngine& engine) {
	if (!lockActive_ || !awaitingStart_) {
		return false;
	}
	restorePaused(engine);
	return true;
}

void CreationPauseStateMachine::restorePaused(sim::SimulationEngine& engine) {
	engine.setPaused(pausedBeforeCreation_);
	lockActive_ = false;
	awaitingStart_ = false;
	pauseAckToken_.reset();
	pendingStartPixel_.reset();
	autoStartPixel_.reset();
}

void CreationPauseStateMachine::tick(bool creationEnabled,
                                     bool creationInProgress,
                                     sim::SimulationEngine& engine) {
	autoStartPixel_.reset();
	if (!creationEnabled && lockActive_) {
		restorePaused(engine);
		return;
	}

	if (creationInProgress && !lockActive_) {
		pausedBeforeCreation_ = engine.config().paused;
		lockActive_ = true;
		awaitingStart_ = false;
		pauseAckToken_.reset();
		pendingStartPixel_.reset();
	}

	if (!lockActive_) {
		return;
	}

	engine.setPaused(true);

	if (awaitingStart_ && pauseAckToken_.has_value() && pendingStartPixel_.has_value() &&
	    !creationInProgress && engine.isPauseAcked(*pauseAckToken_)) {
		autoStartPixel_ = pendingStartPixel_;
		pendingStartPixel_.reset();
		pauseAckToken_.reset();
		awaitingStart_ = false;
	}

	if (!creationInProgress && !awaitingStart_) {
		if (autoStartPixel_.has_value()) {
			return;
		}
		restorePaused(engine);
	}
}

}  // namespace ui
