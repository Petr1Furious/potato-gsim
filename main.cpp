#include "io/NativeFileDialog.hpp"
#include "io/Persistence.hpp"
#include "render/Renderer.hpp"
#include "scenario/ScenarioManager.hpp"
#include "sim/SimulationEngine.hpp"
#include "ui/CreationPauseStateMachine.hpp"
#include "ui/InspectorOverlay.hpp"
#include "ui/QuantityFormat.hpp"
#include "ui/UiState.hpp"

#include <SFML/Graphics.hpp>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstddef>
#include <iomanip>
#include <optional>
#include <sstream>
#include <string>
#include <vector>

namespace {

double round3(double v) {
	return std::round(v * 1000.0) / 1000.0;
}

std::string formatScientific(double value, int precision = 3) {
	std::ostringstream ss;
	ss << std::scientific << std::setprecision(precision) << value;
	return ss.str();
}

std::string formatFixed(double value, int precision = 3) {
	std::ostringstream ss;
	ss << std::fixed << std::setprecision(precision) << value;
	return ss.str();
}

void fitViewToBodies(render::Renderer& renderer, const std::vector<sim::SpawnCommand>& bodies) {
	if (bodies.empty()) {
		return;
	}
	double minX = bodies.front().x;
	double maxX = bodies.front().x;
	double minY = bodies.front().y;
	double maxY = bodies.front().y;
	for (const sim::SpawnCommand& b : bodies) {
		minX = std::min(minX, b.x - b.radius);
		maxX = std::max(maxX, b.x + b.radius);
		minY = std::min(minY, b.y - b.radius);
		maxY = std::max(maxY, b.y + b.radius);
	}
	const float cx = static_cast<float>((minX + maxX) * 0.5);
	const float cy = static_cast<float>((minY + maxY) * 0.5);
	const float sx = static_cast<float>(std::max(1.0, (maxX - minX) * 1.25));
	const float sy = static_cast<float>(std::max(1.0, (maxY - minY) * 1.25));
	renderer.setViewCenter(sf::Vector2f(cx, cy));
	renderer.setViewSize(sf::Vector2f(sx, sy));
}

void setWindowFullscreen(sf::RenderWindow& window, bool fullscreen) {
	if (fullscreen) {
		const auto modes = sf::VideoMode::getFullscreenModes();
		const sf::VideoMode mode = modes.empty() ? sf::VideoMode({1920, 1080}) : modes.front();
		window.create(mode, "potato_gsim", sf::Style::Default, sf::State::Fullscreen);
	} else {
		window.create(sf::VideoMode({1400, 900}), "potato_gsim", sf::Style::Default,
		              sf::State::Windowed);
	}
	window.setVerticalSyncEnabled(true);
	window.setFramerateLimit(0);
}

}  // namespace

int main() {
	sf::RenderWindow window(sf::VideoMode({1400, 900}), "potato_gsim", sf::Style::Default,
	                        sf::State::Windowed);
	window.setVerticalSyncEnabled(true);
	window.setFramerateLimit(0);

	sim::SimulationConfig config;
	config.mode = sim::SimulationMode::RealTimeVariableStep;
	config.timeScale = 3600.0;
	config.fixedDtSeconds = 1.0 / 240.0;
	config.realtimeDtWindowSize = 120;
	config.realtimeDtSpikeClampMultiplier = 6.0;
	config.realtimeDtClampWarmupSamples = 16;
	config.gravitationalConstant = 6.67430e-11;
	config.softeningEpsilon = 1.0e6;
	config.barnesHutTheta = 0.6;
	config.collisionCellScale = 8.0;
	config.workerCount = 0;

	sim::SimulationEngine engine(config);
	engine.start();

	render::Renderer renderer(window);
	ui::InspectorOverlay overlay;
	ui::UiState ui;
	ui.presetNames = scenario::ScenarioManager::presetNames();
	ui.creation.setDensity(5514.0);
	ui.traces.setSettings(ui::TraceStore::Settings{
	    .enabled = false,
	    .relative = false,
	    .maxPointsPerBody = 260,
	});
	ui.predictor.setSettings(ui::Predictor::Settings{
	    .enabled = true,
	    .steps = 220,
	    .dt = 1.0 / 120.0,
	    .realTimeHorizonSeconds = 5.0,
	    .maxAttractors = 1500,
	});
	engine.setDebugMetricsEnabled(ui.showDebugInfo);

	auto applyScenario = [&](const std::vector<sim::SpawnCommand>& bodies) {
		engine.queueReplaceWorld(bodies);
		engine.resetSimulationTimer();
		ui.selection.clear();
		ui.traces.clear();
		ui.creation.cancel();
		fitViewToBodies(renderer, bodies);
	};
	applyScenario(scenario::ScenarioManager::makePreset(ui.presetIndex));

	bool middlePanning = false;
	sf::Vector2i lastPanPixel{0, 0};
	bool leftPanning = false;
	bool leftDragMaySelect = false;
	sf::Vector2i leftPanPixel{0, 0};
	sf::Vector2i leftDownPixel{0, 0};
	bool fullscreen = false;
	bool wasPausedBeforeMenu = false;
	std::vector<sim::BodySnapshot> bodies;
	std::vector<std::pair<sim::BodyId, sim::BodyId>> mergeRemapEvents;
	std::vector<sf::Vector2f> selectedPrediction;
	std::vector<sf::Vector2f> selectedPredictionOffsets;
	std::vector<sf::Vector2f> creationPrediction;
	std::vector<sf::Vector2f> creationPredictionOffsets;
	std::optional<sim::BodyId> selectedPredictionBodyId;
	std::optional<sim::BodyId> creationPredictionRelativeSelectedId;
	std::optional<sim::BodySnapshot> selectedBodyForInput;
	std::optional<sim::BodyId> trackedFollowId;
	std::optional<sf::Vector2f> followViewOffset;

	auto statusUntil = std::chrono::steady_clock::now();
	auto setStatus = [&](const std::string& message, double seconds) {
		ui.statusMessage = message;
		statusUntil = std::chrono::steady_clock::now() +
		              std::chrono::milliseconds(static_cast<int>(seconds * 1000.0));
	};
	ui::CreationPauseStateMachine creationPauseState;

	auto openMenu = [&](bool active) {
		if (ui.menu.active() == active) {
			return;
		}
		if (active) {
			wasPausedBeforeMenu = engine.config().paused;
			engine.setPaused(true);
		} else {
			engine.setPaused(wasPausedBeforeMenu);
		}
		ui.menu.setActive(active);
	};

	auto dispatchPersistenceSave = [&]() {
		const std::optional<std::string> path =
		    io::NativeFileDialog::pickSavePath("Save Simulation", "world", "pgsim");
		if (!path.has_value()) {
			return;
		}

		engine.copyBodies(bodies);
		io::PersistedWorldState save;
		save.simConfig = engine.config();
		save.simulationTimeSeconds = engine.simulationTimeSeconds();
		save.ui.trailsEnabled = ui.traces.settings().enabled;
		save.ui.relativeTrails = ui.traces.settings().relative;
		save.ui.predictionsEnabled = ui.showPredictions;
		save.ui.followSelected = ui.selection.followEnabled();
		save.ui.creationRelativeFrame = ui.creation.relativeFrame();
		save.ui.negativeMass = ui.creation.negativeMass();
		save.ui.creationDensity = ui.creation.density();
		save.ui.predictionSteps = ui.predictor.settings().steps;
		save.ui.predictionDt = ui.predictor.settings().dt;
		save.ui.presetIndex = ui.presetIndex;
		save.camera.center = renderer.view().getCenter();
		save.camera.size = renderer.view().getSize();
		save.bodies.reserve(bodies.size());
		for (const sim::BodySnapshot& b : bodies) {
			save.bodies.push_back(sim::SpawnCommand{
			    .x = b.x,
			    .y = b.y,
			    .vx = b.vx,
			    .vy = b.vy,
			    .mass = b.mass,
			    .radius = b.radius,
			});
		}

		std::string error;
		if (io::Persistence::save(*path, save, error)) {
			setStatus("Saved to " + *path, 3.5);
		} else {
			setStatus("Save failed: " + error, 4.5);
		}
	};

	auto dispatchPersistenceLoad = [&]() {
		const std::optional<std::string> path =
		    io::NativeFileDialog::pickOpenPath("Load Simulation", "pgsim");
		if (!path.has_value()) {
			return;
		}
		io::PersistedWorldState loaded;
		std::string error;
		if (!io::Persistence::load(*path, loaded, error)) {
			setStatus("Load failed: " + error, 4.5);
			return;
		}

		engine.queueReplaceWorld(loaded.bodies);
		engine.setMode(loaded.simConfig.mode);
		engine.setTimeScale(loaded.simConfig.timeScale);
		engine.setFixedDt(loaded.simConfig.fixedDtSeconds);
		engine.setRealtimeDtOutlierClamp(loaded.simConfig.realtimeDtWindowSize,
		                                 loaded.simConfig.realtimeDtSpikeClampMultiplier,
		                                 loaded.simConfig.realtimeDtClampWarmupSamples);
		engine.setGravityConstant(loaded.simConfig.gravitationalConstant);
		engine.setSoftening(loaded.simConfig.softeningEpsilon);
		engine.setTheta(loaded.simConfig.barnesHutTheta);
		engine.setCollisionCellScale(loaded.simConfig.collisionCellScale);
		engine.setWorkerCount(loaded.simConfig.workerCount);
		engine.setSimulationTimer(loaded.simulationTimeSeconds);
		engine.setPaused(loaded.simConfig.paused);

		ui.creation.setDensity(loaded.ui.creationDensity);
		ui.creation.setNegativeMass(loaded.ui.negativeMass);
		if (ui.creation.relativeFrame() != loaded.ui.creationRelativeFrame) {
			ui.creation.toggleRelativeFrame();
		}
		ui.selection.setFollow(loaded.ui.followSelected);
		ui.showPredictions = loaded.ui.predictionsEnabled;
		auto traceSettings = ui.traces.settings();
		traceSettings.enabled = loaded.ui.trailsEnabled;
		traceSettings.relative = loaded.ui.relativeTrails;
		ui.traces.setSettings(traceSettings);
		auto predictorSettings = ui.predictor.settings();
		predictorSettings.steps = loaded.ui.predictionSteps;
		predictorSettings.dt = loaded.ui.predictionDt;
		ui.predictor.setSettings(predictorSettings);
		ui.presetIndex = loaded.ui.presetIndex;
		renderer.setViewCenter(loaded.camera.center);
		renderer.setViewSize(loaded.camera.size);
		ui.selection.clear();
		ui.traces.clear();
		setStatus("Loaded " + *path, 3.5);
	};

	auto dispatchAction = [&](ui::Action action) {
		switch (action) {
			case ui::Action::ToggleMenu:
				openMenu(!ui.menu.active());
				break;
			case ui::Action::TogglePause:
				if (creationPauseState.blocksPauseToggle()) {
					break;
				}
				engine.togglePaused();
				break;
			case ui::Action::ToggleMode:
				engine.toggleMode();
				break;
			case ui::Action::ToggleFullscreen:
				fullscreen = !fullscreen;
				setWindowFullscreen(window, fullscreen);
				renderer.onResize(window.getSize());
				break;
			case ui::Action::ResetView:
				renderer.resetView();
				break;
			case ui::Action::ToggleFollow:
				ui.selection.toggleFollow();
				break;
			case ui::Action::ToggleTrails: {
				auto s = ui.traces.settings();
				s.enabled = !s.enabled;
				ui.traces.setSettings(s);
				if (!s.enabled) {
					ui.traces.clear();
				}
			} break;
			case ui::Action::ToggleRelativeTrails: {
				auto s = ui.traces.settings();
				s.relative = !s.relative;
				ui.traces.setSettings(s);
			} break;
			case ui::Action::TogglePredictions:
				ui.showPredictions = !ui.showPredictions;
				break;
			case ui::Action::ToggleLabels:
				ui.showHoverLabels = !ui.showHoverLabels;
				break;
			case ui::Action::ToggleDebugInfo:
				ui.showDebugInfo = !ui.showDebugInfo;
				engine.setDebugMetricsEnabled(ui.showDebugInfo);
				setStatus(
				    std::string("Debug overlay ") + (ui.showDebugInfo ? "enabled" : "disabled"),
				    1.8);
				break;
			case ui::Action::ToggleCreationTool:
				ui.creation.toggleEnabled();
				break;
			case ui::Action::ToggleRelativeCreationFrame:
				ui.creation.toggleRelativeFrame();
				break;
			case ui::Action::ToggleNegativeMass:
				ui.creation.toggleNegativeMass();
				break;
			case ui::Action::ResetTimer:
				engine.resetSimulationTimer();
				break;
			case ui::Action::AdvancedResetFrame: {
				const std::optional<sim::BodyId> selected = ui.selection.selectedId();
				if (!selected.has_value()) {
					break;
				}
				const std::optional<sim::BodySnapshot> selectedBody = engine.bodyById(*selected);
				if (!selectedBody.has_value()) {
					break;
				}
				engine.copyBodies(bodies);
				std::vector<sim::SpawnCommand> transformed;
				transformed.reserve(bodies.size());
				for (const sim::BodySnapshot& b : bodies) {
					transformed.push_back(sim::SpawnCommand{
					    .x = b.x - selectedBody->x,
					    .y = b.y - selectedBody->y,
					    .vx = b.vx - selectedBody->vx,
					    .vy = b.vy - selectedBody->vy,
					    .mass = b.mass,
					    .radius = b.radius,
					});
				}
				engine.queueReplaceWorld(transformed);
				ui.selection.clear();
				ui.traces.clear();
			} break;
			case ui::Action::SaveWorld:
				dispatchPersistenceSave();
				break;
			case ui::Action::LoadWorld:
				dispatchPersistenceLoad();
				break;
			case ui::Action::RandomScenario:
				applyScenario(scenario::ScenarioManager::makeRandom(10000, 0.0, 0.0, 1e11));
				break;
			case ui::Action::ClearScenario:
				engine.queueClearAll();
				ui.selection.clear();
				ui.traces.clear();
				break;
			case ui::Action::NextPreset:
				if (!ui.presetNames.empty()) {
					ui.presetIndex = (ui.presetIndex + 1) % ui.presetNames.size();
					applyScenario(scenario::ScenarioManager::makePreset(ui.presetIndex));
				}
				break;
			case ui::Action::PrevPreset:
				if (!ui.presetNames.empty()) {
					ui.presetIndex =
					    (ui.presetIndex + ui.presetNames.size() - 1) % ui.presetNames.size();
					applyScenario(scenario::ScenarioManager::makePreset(ui.presetIndex));
				}
				break;
		}
	};

	auto dispatchMenuAction = [&](const ui::MenuAction& action) {
		const sim::SimulationConfig cfg = engine.config();
		if (action.itemId == "sim.pause") {
			if (creationPauseState.blocksPauseToggle()) {
				return;
			}
			engine.togglePaused();
		} else if (action.itemId == "sim.mode") {
			engine.toggleMode();
		} else if (action.itemId == "sim.timescale" && action.adjustDelta != 0) {
			engine.scaleTimeBy(action.adjustDelta > 0 ? 1.2 : (1.0 / 1.2));
		} else if (action.itemId == "sim.theta" && action.adjustDelta != 0) {
			engine.setTheta(cfg.barnesHutTheta + action.adjustDelta * 0.03);
		} else if (action.itemId == "sim.softening" && action.adjustDelta != 0) {
			engine.setSoftening(cfg.softeningEpsilon * (action.adjustDelta > 0 ? 1.1 : 0.9));
		} else if (action.itemId == "sim.gravity" && action.adjustDelta != 0) {
			engine.setGravityConstant(cfg.gravitationalConstant *
			                          (action.adjustDelta > 0 ? 1.1 : 0.9));
		} else if (action.itemId == "sim.workers" && action.adjustDelta != 0) {
			const std::size_t workers = action.adjustDelta > 0
			                                ? (cfg.workerCount + 1)
			                                : (cfg.workerCount > 0 ? cfg.workerCount - 1 : 0);
			engine.setWorkerCount(workers);
		} else if (action.itemId == "creation.enabled") {
			ui.creation.toggleEnabled();
		} else if (action.itemId == "creation.density" && action.adjustDelta != 0) {
			ui.creation.scaleDensity(action.adjustDelta > 0 ? 1.2 : (1.0 / 1.2));
		} else if (action.itemId == "creation.negmass") {
			ui.creation.toggleNegativeMass();
		} else if (action.itemId == "creation.relframe") {
			ui.creation.toggleRelativeFrame();
		} else if (action.itemId == "trace.enabled") {
			auto s = ui.traces.settings();
			s.enabled = !s.enabled;
			ui.traces.setSettings(s);
			if (!s.enabled) {
				ui.traces.clear();
			}
		} else if (action.itemId == "trace.relative") {
			auto s = ui.traces.settings();
			s.relative = !s.relative;
			ui.traces.setSettings(s);
		} else if (action.itemId == "predict.enabled") {
			ui.showPredictions = !ui.showPredictions;
		} else if (action.itemId == "labels.toggle") {
			ui.showHoverLabels = !ui.showHoverLabels;
		} else if (action.itemId == "debug.toggle") {
			dispatchAction(ui::Action::ToggleDebugInfo);
		} else if (action.itemId == "predict.realtime_window" && action.adjustDelta != 0) {
			auto predictorSettings = ui.predictor.settings();
			const double factor = action.adjustDelta > 0 ? 1.15 : (1.0 / 1.15);
			predictorSettings.realTimeHorizonSeconds =
			    std::clamp(predictorSettings.realTimeHorizonSeconds * factor, 0.05, 120.0);
			ui.predictor.setSettings(predictorSettings);
		} else if (action.itemId == "predict.steps" && action.adjustDelta != 0) {
			auto predictorSettings = ui.predictor.settings();
			predictorSettings.steps =
			    std::clamp(predictorSettings.steps + (action.adjustDelta > 0 ? 20 : -20), 20, 5000);
			ui.predictor.setSettings(predictorSettings);
		} else if (action.itemId == "action.save") {
			dispatchPersistenceSave();
		} else if (action.itemId == "action.load") {
			dispatchPersistenceLoad();
		} else if (action.itemId == "action.random") {
			dispatchAction(ui::Action::RandomScenario);
		} else if (action.itemId == "action.clear") {
			dispatchAction(ui::Action::ClearScenario);
		} else if (action.itemId == "action.prevpreset") {
			dispatchAction(ui::Action::PrevPreset);
		} else if (action.itemId == "action.nextpreset") {
			dispatchAction(ui::Action::NextPreset);
		} else if (action.itemId == "action.resettimer") {
			dispatchAction(ui::Action::ResetTimer);
		} else if (action.itemId == "action.resetframe") {
			dispatchAction(ui::Action::AdvancedResetFrame);
		}
	};

	auto lastFrameTime = std::chrono::steady_clock::now();
	auto lastPredictTime = lastFrameTime;
	double fps = 0.0;
	const auto currentSimSecondsPerRealSecond = [&engine]() {
		const sim::SimulationConfig liveCfg = engine.config();
		// While paused, measured throughput is stale by definition; always derive
		// from current config so time-scale edits immediately affect UI/tools.
		if (!liveCfg.paused) {
			const double measured = engine.simulatedSecondsPerRealSecond();
			if (std::isfinite(measured) && measured > 0.0) {
				return measured;
			}
		}
		if (!std::isfinite(liveCfg.timeScale) || liveCfg.timeScale <= 0.0) {
			return 1.0;
		}
		if (liveCfg.mode == sim::SimulationMode::DeterministicFixedStep) {
			return liveCfg.timeScale * std::max(1.0, engine.updatesPerSecond());
		}
		return liveCfg.timeScale;
	};
	const auto panView = [&](const sf::Vector2i& pixelDelta) {
		const sf::Vector2f before = renderer.view().getCenter();
		renderer.panByPixels(pixelDelta);
		if (ui.selection.followEnabled() && followViewOffset.has_value()) {
			const sf::Vector2f after = renderer.view().getCenter();
			*followViewOffset += (after - before);
		}
	};

	while (window.isOpen()) {
		ui.creation.setFollowReferenceFrame(ui.selection.followEnabled());

		const auto now = std::chrono::steady_clock::now();
		const double frameDt = std::chrono::duration<double>(now - lastFrameTime).count();
		lastFrameTime = now;
		if (frameDt > 1e-6) {
			const double instantFps = 1.0 / frameDt;
			fps = (fps == 0.0) ? instantFps : (fps * 0.9 + instantFps * 0.1);
		}

		while (const auto event = window.pollEvent()) {
			if (event->is<sf::Event::Closed>()) {
				window.close();
				continue;
			}
			if (const auto* resized = event->getIf<sf::Event::Resized>()) {
				renderer.onResize(resized->size);
			}
			if (const auto* wheel = event->getIf<sf::Event::MouseWheelScrolled>()) {
				if (!ui.menu.active()) {
					renderer.zoomAtPixel(wheel->position, wheel->delta);
				}
			}
			if (const auto* moved = event->getIf<sf::Event::MouseMoved>()) {
				if (middlePanning && !ui.menu.active()) {
					const sf::Vector2i delta = moved->position - lastPanPixel;
					panView(delta);
					lastPanPixel = moved->position;
				}
				if (leftPanning && !ui.menu.active() && !ui.creation.enabled()) {
					const sf::Vector2i delta = moved->position - leftPanPixel;
					panView(delta);
					leftPanPixel = moved->position;
					const float dragDx = static_cast<float>(moved->position.x - leftDownPixel.x);
					const float dragDy = static_cast<float>(moved->position.y - leftDownPixel.y);
					if (std::sqrt(dragDx * dragDx + dragDy * dragDy) > 10.0f) {
						leftDragMaySelect = false;
					}
				}
				ui.creation.updateCursor(renderer.screenToWorld(moved->position));
			}
			if (const auto* mousePressed = event->getIf<sf::Event::MouseButtonPressed>()) {
				if (ui.menu.active()) {
					if (mousePressed->button == sf::Mouse::Button::Left) {
						const std::optional<ui::MenuAction> menuAction =
						    ui.menu.handleMouseClick(mousePressed->position, window.getSize());
						if (menuAction.has_value()) {
							dispatchMenuAction(*menuAction);
						}
					}
					continue;
				}

				if (mousePressed->button == sf::Mouse::Button::Middle) {
					middlePanning = true;
					lastPanPixel = mousePressed->position;
					continue;
				}

				const sf::Vector2f world = renderer.screenToWorld(mousePressed->position);
				if (mousePressed->button == sf::Mouse::Button::Left) {
					if (ui.creation.enabled()) {
						if (creationPauseState.onCreationClickStart(
						        mousePressed->position, ui.creation.enabled(),
						        ui.creation.isInProgress(), engine)) {
							continue;
						}
						std::optional<sim::BodySnapshot> selectedBody;
						if (selectedBodyForInput.has_value() &&
						    ui.selection.selectedId().has_value() &&
						    selectedBodyForInput->id == *ui.selection.selectedId() &&
						    (ui.creation.relativeFrame() || ui.selection.followEnabled())) {
							selectedBody = selectedBodyForInput;
						}
						const std::optional<sim::SpawnCommand> spawn = ui.creation.handleLeftClick(
						    world, selectedBody, currentSimSecondsPerRealSecond());
						if (spawn.has_value()) {
							engine.queueSpawn(*spawn);
							if (ui.creation.negativeMass() && ui.showNegativeMassWarning) {
								setStatus("Negative-mass spawn created (advanced mode).", 3.0);
							}
						}
					} else {
						leftPanning = true;
						leftDragMaySelect = true;
						leftPanPixel = mousePressed->position;
						leftDownPixel = mousePressed->position;
					}
				} else if (mousePressed->button == sf::Mouse::Button::Right) {
					const double pickRadius = std::max(1.0f, renderer.worldUnitsPerPixel() * 20.0f);
					engine.queueDeleteNearest(world.x, world.y, pickRadius);
				}
			}
			if (const auto* mouseReleased = event->getIf<sf::Event::MouseButtonReleased>()) {
				if (mouseReleased->button == sf::Mouse::Button::Middle) {
					middlePanning = false;
				}
				if (mouseReleased->button == sf::Mouse::Button::Left) {
					if (leftPanning && leftDragMaySelect && !ui.menu.active() &&
					    !ui.creation.enabled()) {
						const sf::Vector2f world = renderer.screenToWorld(mouseReleased->position);
						const auto worldToScreenDistance = [&](const sf::Vector2f& delta) -> float {
							const sf::Vector2i p0 = renderer.worldToPixel(sf::Vector2f(0.0f, 0.0f));
							const sf::Vector2i p1 = renderer.worldToPixel(delta);
							const sf::Vector2f diff(static_cast<float>(p1.x - p0.x),
							                        static_cast<float>(p1.y - p0.y));
							return std::sqrt(diff.x * diff.x + diff.y * diff.y);
						};
						const std::optional<sim::BodyId> picked =
						    ui.selection.pick(engine, world, worldToScreenDistance, 26.0f);
						if (picked.has_value()) {
							ui.selection.setSelected(*picked);
						} else {
							ui.selection.clear();
						}
					}
					leftPanning = false;
					leftDragMaySelect = false;
				}
			}
			if (const auto* key = event->getIf<sf::Event::KeyPressed>()) {
				if (ui.menu.active()) {
					if (key->code == sf::Keyboard::Key::Escape) {
						openMenu(false);
						continue;
					}
					if (key->code == sf::Keyboard::Key::Up) {
						ui.menu.moveSelection(-1);
						continue;
					}
					if (key->code == sf::Keyboard::Key::Down) {
						ui.menu.moveSelection(1);
						continue;
					}
					const std::optional<ui::MenuAction> action = ui.menu.handleKeyPress(key->code);
					if (action.has_value()) {
						dispatchMenuAction(*action);
					}
					continue;
				}

				if (key->code == sf::Keyboard::Key::Escape) {
					if (ui.creation.isInProgress()) {
						ui.creation.cancel();
						setStatus("Creation cancelled", 1.6);
						continue;
					}
					if (creationPauseState.onEscape(engine)) {
						continue;
					}
				}

				const std::optional<ui::Action> action =
				    ui.input.mapKeyPress(key->code, ui.menu.active());
				if (action.has_value()) {
					dispatchAction(*action);
				}
			}
		}
		const bool creationInProgress = ui.creation.enabled() && ui.creation.isInProgress();
		creationPauseState.tick(ui.creation.enabled(), creationInProgress, engine);

		const ui::HoldAdjustments holds = ui.input.computeHolds(frameDt, !ui.menu.active());
		if (holds.timeScaleFactor != 1.0) {
			engine.scaleTimeBy(holds.timeScaleFactor);
		}
		if (holds.densityFactor != 1.0) {
			ui.creation.scaleDensity(holds.densityFactor);
		}
		if (holds.predictionFactor != 1.0) {
			ui.predictor.scaleHorizon(holds.predictionFactor);
		}
		if (holds.panPixelsX != 0.0 || holds.panPixelsY != 0.0) {
			panView(sf::Vector2i(static_cast<int>(std::lround(holds.panPixelsX)),
			                     static_cast<int>(std::lround(holds.panPixelsY))));
		}
		const sf::Vector2f viewCenterBeforeUpdate = renderer.view().getCenter();
		renderer.update(frameDt);
		if (ui.selection.followEnabled() && followViewOffset.has_value()) {
			const sf::Vector2f viewCenterAfterUpdate = renderer.view().getCenter();
			*followViewOffset += (viewCenterAfterUpdate - viewCenterBeforeUpdate);
		}

		const sim::SimulationConfig cfg = engine.config();
		const sim::EngineDebugStats debugStats = engine.debugStats();

		engine.drainMergeRemapEvents(mergeRemapEvents);
		ui.selection.applyMergeRemap(mergeRemapEvents);
		ui.traces.applyMergeRemap(mergeRemapEvents);
		ui.selection.validateAgainstEngine(engine);

		engine.copyBodies(bodies);
		std::optional<sim::BodySnapshot> selectedBody;
		if (ui.selection.selectedId().has_value()) {
			const sim::BodyId selectedId = *ui.selection.selectedId();
			const auto it = std::find_if(
			    bodies.begin(), bodies.end(),
			    [selectedId](const sim::BodySnapshot& b) { return b.id == selectedId; });
			if (it != bodies.end()) {
				selectedBody = *it;
			} else {
				ui.selection.clear();
			}
		}
		const std::optional<sim::BodySnapshot> creationReferenceBody =
		    (selectedBody.has_value() &&
		     (ui.creation.relativeFrame() || ui.selection.followEnabled()))
		        ? selectedBody
		        : std::nullopt;
		if (ui.selection.followEnabled() && selectedBody.has_value()) {
			const sf::Vector2f currentSelectedPos(static_cast<float>(selectedBody->x),
			                                      static_cast<float>(selectedBody->y));
			if (!trackedFollowId.has_value() || *trackedFollowId != selectedBody->id ||
			    !followViewOffset.has_value()) {
				followViewOffset = renderer.view().getCenter() - currentSelectedPos;
			}
			renderer.setViewCenter(currentSelectedPos + *followViewOffset);
			trackedFollowId = selectedBody->id;
		} else {
			trackedFollowId.reset();
			followViewOffset.reset();
		}
		const std::optional<sim::BodySnapshot> selectedForCreation =
		    (selectedBody.has_value() &&
		     (ui.creation.relativeFrame() || ui.selection.followEnabled()))
		        ? selectedBody
		        : std::nullopt;
		if (const std::optional<sf::Vector2i> autoStartPixel =
		        creationPauseState.consumeAutoStartPixel();
		    autoStartPixel.has_value() && !ui.creation.isInProgress()) {
			const sf::Vector2f startWorld = renderer.screenToWorld(*autoStartPixel);
			(void)ui.creation.handleLeftClick(startWorld, selectedForCreation,
			                                  currentSimSecondsPerRealSecond());
		}
		ui.creation.updateCursor(renderer.screenToWorld(sf::Mouse::getPosition(window)),
		                         selectedForCreation);
		selectedBodyForInput = selectedBody;
		ui.traces.ingest(bodies, engine.simulationTimeSeconds());
		const sf::Vector2f cursorWorld = renderer.screenToWorld(sf::Mouse::getPosition(window));
		std::optional<sim::BodySnapshot> hoveredBody;
		const double worldUnitsPerPixel =
		    std::max(1e-9, static_cast<double>(renderer.worldUnitsPerPixel()));
		const double hoverMaxDistancePx = 26.0;
		double bestSurfaceDistancePx = hoverMaxDistancePx;
		double bestMass = -1.0;
		for (const sim::BodySnapshot& b : bodies) {
			if (selectedBody.has_value() && b.id == selectedBody->id) {
				continue;
			}
			const double dx = b.x - cursorWorld.x;
			const double dy = b.y - cursorWorld.y;
			const double centerDistance = std::sqrt(dx * dx + dy * dy);
			const double surfaceDistancePx =
			    std::max(0.0, centerDistance - b.radius) / worldUnitsPerPixel;
			if (surfaceDistancePx > hoverMaxDistancePx) {
				continue;
			}

			const double massAbs = std::abs(b.mass);
			constexpr double kDistanceBiasPx = 2.0;
			const bool betterDistance = surfaceDistancePx + kDistanceBiasPx < bestSurfaceDistancePx;
			const bool closeEnoughDistance =
			    std::abs(surfaceDistancePx - bestSurfaceDistancePx) <= kDistanceBiasPx;
			const bool betterMassTieBreak = closeEnoughDistance && (massAbs > bestMass);
			if (betterDistance || betterMassTieBreak) {
				bestSurfaceDistancePx = surfaceDistancePx;
				bestMass = massAbs;
				hoveredBody = b;
			}
		}

		auto predictorSettings = ui.predictor.settings();
		const double displayTimeRate = std::max(1e-6, currentSimSecondsPerRealSecond());
		predictorSettings.realTimeHorizonSeconds =
		    std::clamp(predictorSettings.realTimeHorizonSeconds, 0.05, 120.0);
		predictorSettings.steps = std::clamp(predictorSettings.steps, 20, 5000);
		const double predictWindowSimSeconds =
		    predictorSettings.realTimeHorizonSeconds * displayTimeRate;
		predictorSettings.dt =
		    std::max(1e-9, predictWindowSimSeconds / static_cast<double>(predictorSettings.steps));
		ui.predictor.setSettings(predictorSettings);
		const double simSecondsPerUpdate =
		    (engine.simulatedSecondsPerUpdate() > 0.0)
		        ? engine.simulatedSecondsPerUpdate()
		        : ((cfg.mode == sim::SimulationMode::DeterministicFixedStep)
		               ? cfg.timeScale
		               : predictorSettings.dt);
		const std::string timeScaleDisplay =
		    (cfg.mode == sim::SimulationMode::DeterministicFixedStep)
		        ? (ui::formatTimeLegacy(displayTimeRate) + "/s (" +
		           ui::formatTimeLegacy(simSecondsPerUpdate) + "/U)")
		        : (ui::formatTimeLegacy(cfg.timeScale) + "/s");
		const std::string predictWindowDisplay =
		    formatFixed(predictorSettings.realTimeHorizonSeconds, 2) + " s real (" +
		    ui::formatTimeLegacy(predictWindowSimSeconds) + " sim)";
		const std::string scaleDisplay =
		    ui::formatDistanceLegacy(renderer.worldUnitsPerPixel()) + "/pixel";
		const std::optional<sim::SpawnCommand> creationPreviewSpawn =
		    ui.creation.previewSpawn(creationReferenceBody, currentSimSecondsPerRealSecond());

		if (ui.showPredictions &&
		    std::chrono::duration<double>(now - lastPredictTime).count() > 0.12) {
			lastPredictTime = now;
			selectedPredictionOffsets.clear();
			selectedPredictionBodyId.reset();
			creationPredictionOffsets.clear();
			creationPredictionRelativeSelectedId.reset();
			if (selectedBody.has_value()) {
				const sim::SimulationConfig cfg = engine.config();
				const std::vector<sf::Vector2f> selectedPredicted = ui.predictor.predictForBody(
				    bodies, selectedBody->id, cfg.gravitationalConstant, cfg.softeningEpsilon);
				if (!selectedPredicted.empty()) {
					const sf::Vector2f anchor(static_cast<float>(selectedBody->x),
					                          static_cast<float>(selectedBody->y));
					selectedPredictionOffsets.reserve(selectedPredicted.size());
					for (const sf::Vector2f& point : selectedPredicted) {
						selectedPredictionOffsets.push_back(point - anchor);
					}
					selectedPredictionBodyId = selectedBody->id;
				}
			}
			if (creationPreviewSpawn.has_value() &&
			    ui.creation.step() == ui::CreationTool::Step::SetVelocity) {
				const sim::SimulationConfig cfg = engine.config();
				if (selectedBody.has_value()) {
					creationPredictionOffsets = ui.predictor.predictSpawnRelativeToBody(
					    bodies, *creationPreviewSpawn, selectedBody->id, cfg.gravitationalConstant,
					    cfg.softeningEpsilon);
					creationPredictionRelativeSelectedId = selectedBody->id;
				} else {
					const std::vector<sf::Vector2f> previewPredicted =
					    ui.predictor.predictSpawn(bodies, *creationPreviewSpawn,
					                              cfg.gravitationalConstant, cfg.softeningEpsilon);
					const sf::Vector2f anchor(static_cast<float>(creationPreviewSpawn->x),
					                          static_cast<float>(creationPreviewSpawn->y));
					creationPredictionOffsets.reserve(previewPredicted.size());
					for (const sf::Vector2f& point : previewPredicted) {
						creationPredictionOffsets.push_back(point - anchor);
					}
				}
			}
		}
		selectedPrediction.clear();
		if (ui.showPredictions && selectedBody.has_value() &&
		    selectedPredictionBodyId.has_value() && *selectedPredictionBodyId == selectedBody->id &&
		    !selectedPredictionOffsets.empty()) {
			const sf::Vector2f anchor(static_cast<float>(selectedBody->x),
			                          static_cast<float>(selectedBody->y));
			selectedPrediction.reserve(selectedPredictionOffsets.size());
			for (const sf::Vector2f& offset : selectedPredictionOffsets) {
				selectedPrediction.push_back(anchor + offset);
			}
		}
		creationPrediction.clear();
		if (ui.showPredictions && creationPreviewSpawn.has_value() &&
		    ui.creation.step() == ui::CreationTool::Step::SetVelocity &&
		    !creationPredictionOffsets.empty()) {
			sf::Vector2f anchor(static_cast<float>(creationPreviewSpawn->x),
			                    static_cast<float>(creationPreviewSpawn->y));
			if (creationPredictionRelativeSelectedId.has_value() && selectedBody.has_value() &&
			    *creationPredictionRelativeSelectedId == selectedBody->id) {
				anchor = sf::Vector2f(static_cast<float>(selectedBody->x),
				                      static_cast<float>(selectedBody->y));
			}
			creationPrediction.reserve(creationPredictionOffsets.size());
			for (const sf::Vector2f& offset : creationPredictionOffsets) {
				creationPrediction.push_back(anchor + offset);
			}
		}

		std::vector<ui::MenuItem> menuItems{
		    {"sim.pause", "Simulation Paused", cfg.paused ? "On" : "Off", false},
		    {"sim.mode", "Mode",
		     cfg.mode == sim::SimulationMode::DeterministicFixedStep ? "Deterministic"
		                                                             : "Real-time",
		     false},
		    {"sim.timescale", "Time Speed", timeScaleDisplay, true},
		    {"sim.theta", "Barnes-Hut Theta", formatFixed(cfg.barnesHutTheta), true},
		    {"sim.softening", "Softening Epsilon", ui::formatDistanceLegacy(cfg.softeningEpsilon),
		     true},
		    {"sim.gravity", "Gravity Constant (SI)", formatScientific(cfg.gravitationalConstant),
		     true},
		    {"sim.workers", "Worker Count (0=auto)", std::to_string(cfg.workerCount), true},
		    {"creation.enabled", "Creation Tool", ui.creation.enabled() ? "On" : "Off", false},
		    {"creation.density", "Creation Density (kg/m^3)", formatFixed(ui.creation.density(), 1),
		     true},
		    {"creation.negmass", "Negative Mass (Advanced)",
		     ui.creation.negativeMass() ? "On" : "Off", false},
		    {"creation.relframe", "Creation Relative Frame",
		     ui.creation.relativeFrame() ? "Selected" : "World", false},
		    {"trace.enabled", "Trails", ui.traces.settings().enabled ? "On" : "Off", false},
		    {"trace.relative", "Relative Trails", ui.traces.settings().relative ? "On" : "Off",
		     false},
		    {"predict.enabled", "Predictions", ui.showPredictions ? "On" : "Off", false},
		    {"labels.toggle", "Hover Labels", ui.showHoverLabels ? "On" : "Off", false},
		    {"debug.toggle", "Debug Overlay", ui.showDebugInfo ? "On" : "Off", false},
		    {"predict.realtime_window", "Prediction Horizon (Real Time)", predictWindowDisplay,
		     true},
		    {"predict.steps", "Prediction Steps", std::to_string(predictorSettings.steps), true},
		    {"action.save", "Save World", "F9", false},
		    {"action.load", "Load World", "F10", false},
		    {"action.random", "Random Scenario", "F5", false},
		    {"action.clear", "Clear World", "F6", false},
		    {"action.prevpreset", "Previous Preset", "F7", false},
		    {"action.nextpreset", "Next Preset", "F8", false},
		    {"action.resettimer", "Reset Timer", "J", false},
		    {"action.resetframe", "Reset Frame of Reference", "Q", false},
		};
		ui.menu.setItems(menuItems);

		renderer.draw(bodies);
		ui.traces.draw(window, selectedBody);

		if (ui.creation.enabled() && ui.creation.isInProgress()) {
			sf::Vector2f anchor = ui.creation.anchor();
			const float radius = ui.creation.previewRadius();
			sf::Vector2f cursor = ui.creation.cursor();
			sf::CircleShape ghost(radius);
			ghost.setOrigin(sf::Vector2f(radius, radius));
			ghost.setPosition(anchor);
			ghost.setFillColor(sf::Color(255, 255, 255, 35));
			ghost.setOutlineColor(sf::Color(230, 230, 255, 160));
			ghost.setOutlineThickness(std::max(0.04f, radius * 0.08f));
			window.draw(ghost);

			sf::VertexArray arrow(sf::PrimitiveType::Lines, 2);
			arrow[0].position = anchor;
			arrow[1].position = cursor;
			arrow[0].color = sf::Color(255, 170, 120, 210);
			arrow[1].color = sf::Color(255, 170, 120, 210);
			window.draw(arrow);

			if (ui.showPredictions && !creationPrediction.empty()) {
				sf::VertexArray strip(sf::PrimitiveType::LineStrip, creationPrediction.size());
				for (std::size_t i = 0; i < creationPrediction.size(); ++i) {
					strip[i].position = creationPrediction[i];
					strip[i].color = sf::Color(255, 220, 120, 150);
				}
				window.draw(strip);
			}

			const sf::Font* infoFont = overlay.fontPtr();
			if (infoFont != nullptr) {
				constexpr double kPi = 3.14159265358979323846;
				const double radiusMeters = std::max(0.05, static_cast<double>(radius));
				double previewX = anchor.x;
				double previewY = anchor.y;
				double previewMass = ui.creation.density() * ((4.0 / 3.0) * kPi * radiusMeters *
				                                              radiusMeters * radiusMeters);
				if (ui.creation.negativeMass()) {
					previewMass = -previewMass;
				}
				double speed = 0.0;
				if (creationPreviewSpawn.has_value()) {
					previewX = creationPreviewSpawn->x;
					previewY = creationPreviewSpawn->y;
					previewMass = creationPreviewSpawn->mass;
					speed = std::sqrt(creationPreviewSpawn->vx * creationPreviewSpawn->vx +
					                  creationPreviewSpawn->vy * creationPreviewSpawn->vy) *
					        currentSimSecondsPerRealSecond();
				}
				double distanceToSelected = 0.0;
				bool hasDistance = false;
				if (selectedBody.has_value()) {
					const double dx = previewX - selectedBody->x;
					const double dy = previewY - selectedBody->y;
					distanceToSelected = std::sqrt(dx * dx + dy * dy);
					hasDistance = true;
				}

				std::ostringstream mass;
				mass << std::scientific << std::setprecision(3) << previewMass;
				std::string previewInfo = "preview m=" + mass.str() + " kg" +
				                          "\nr=" + ui::formatDistanceLegacy(radiusMeters) +
				                          "\nv=" + ui::formatSpeedLegacy(speed);
				if (hasDistance) {
					previewInfo += "\nd=" + ui::formatDistanceLegacy(distanceToSelected);
				}

				const sf::Vector2i anchorPx = renderer.worldToPixel(anchor);
				const sf::Vector2i radiusEdgePx =
				    renderer.worldToPixel(anchor + sf::Vector2f(radius, 0.0f));
				const float radiusPx =
				    std::max(1.0f, static_cast<float>(std::abs(radiusEdgePx.x - anchorPx.x)));
				sf::Text text(*infoFont, previewInfo, 12);
				text.setFillColor(sf::Color::White);
				text.setOutlineColor(sf::Color::Black);
				text.setOutlineThickness(1.0f);
				const sf::FloatRect bounds = text.getLocalBounds();
				text.setOrigin(
				    sf::Vector2f(bounds.position.x + bounds.size.x * 0.5f, bounds.position.y));

				const sf::View oldView = window.getView();
				window.setView(window.getDefaultView());
				text.setPosition(sf::Vector2f(static_cast<float>(anchorPx.x),
				                              static_cast<float>(anchorPx.y) + radiusPx + 10.0f));
				window.draw(text);
				window.setView(oldView);
			}
		}

		const auto worldToPixel = [&](const sf::Vector2f& world) {
			return renderer.worldToPixel(world);
		};
		overlay.drawWorldSelection(window, selectedBody, std::nullopt, selectedPrediction,
		                           std::nullopt, displayTimeRate, true, false, worldToPixel);
		if (ui.showHoverLabels) {
			overlay.drawWorldSelection(window, hoveredBody, selectedBody, {}, std::nullopt,
			                           displayTimeRate, false, true, worldToPixel);
		}

		if (std::chrono::steady_clock::now() > statusUntil) {
			ui.statusMessage.clear();
		}
		std::vector<std::string> hudLines{
		    "Bodies: " + std::to_string(engine.bodyCount()) + " | Preset: " +
		        (ui.presetNames.empty() ? std::string("n/a")
		                                : ui.presetNames[ui.presetIndex % ui.presetNames.size()]),
		    "Mode: " +
		        std::string(cfg.mode == sim::SimulationMode::DeterministicFixedStep
		                        ? "Deterministic"
		                        : "Real-time") +
		        " | Time speed: " + timeScaleDisplay +
		        " | Timer: " + ui::formatTimeLegacy(engine.simulationTimeSeconds()) + " (" +
		        formatScientific(engine.simulationTimeSeconds()) + " s)",
		    "Scale: " + scaleDisplay + " | Predict time: " + predictWindowDisplay,
		    "Theta: " + formatFixed(cfg.barnesHutTheta) +
		        " | Epsilon: " + ui::formatDistanceLegacy(cfg.softeningEpsilon) +
		        " | G: " + formatScientific(cfg.gravitationalConstant),
		    "Workers: " + std::to_string(cfg.workerCount) + " (0=auto)" +
		        " | UPS: " + std::to_string(round3(engine.updatesPerSecond())) +
		        " | FPS: " + std::to_string(round3(fps)),
		    "Create density(kg/m^3): " + formatFixed(ui.creation.density(), 1) +
		        " | Negative mass: " + std::string(ui.creation.negativeMass() ? "On" : "Off") +
		        " | Relative frame: " +
		        std::string(ui.creation.relativeFrame() ? "Selected" : "World"),
		    "Trails: " + std::string(ui.traces.settings().enabled ? "On" : "Off") +
		        " | Relative trails: " + std::string(ui.traces.settings().relative ? "On" : "Off") +
		        " | Predictions: " + std::string(ui.showPredictions ? "On" : "Off") +
		        " | Horizon steps: " + std::to_string(ui.predictor.settings().steps),
		};
		if (ui.showDebugInfo) {
			const double contrib0 = debugStats.avgContributionsAcc0;
			const double contrib1 = debugStats.avgContributionsAcc1;
			const std::string treeLine =
			    debugStats.usedDirectPath
			        ? ("Debug force path: direct O(N^2) | avg contrib/body: " +
			           formatFixed(contrib0, 1) + "/" + formatFixed(contrib1, 1))
			        : ("Debug tree nodes cur/pred: " + std::to_string(debugStats.currentTreeNodes) +
			           "/" + std::to_string(debugStats.predictedTreeNodes) +
			           " | avg contrib/body: " + formatFixed(contrib0, 1) + "/" +
			           formatFixed(contrib1, 1));
			hudLines.push_back(treeLine);
			hudLines.push_back(
			    "Debug avg node visits acc0/acc1: " + formatFixed(debugStats.avgNodeVisitsAcc0, 1) +
			    "/" + formatFixed(debugStats.avgNodeVisitsAcc1, 1) +
			    " | direct interactions: " + formatFixed(debugStats.avgDirectInteractionsAcc0, 1) +
			    "/" + formatFixed(debugStats.avgDirectInteractionsAcc1, 1));
			hudLines.push_back("Debug avg aggregate approximations acc0/acc1: " +
			                   formatFixed(debugStats.avgAggregateApproximationsAcc0, 1) + "/" +
			                   formatFixed(debugStats.avgAggregateApproximationsAcc1, 1) +
			                   " | overlap pairs: " + std::to_string(debugStats.overlapPairs) +
			                   " | merged bodies: " + std::to_string(debugStats.mergedBodies));
			hudLines.push_back("Debug step ms total/cmd/tree/acc0/tree2: " +
			                   formatFixed(debugStats.totalStepMs, 3) + "/" +
			                   formatFixed(debugStats.commandsMs, 3) + "/" +
			                   formatFixed(debugStats.currentTreeBuildMs, 3) + "/" +
			                   formatFixed(debugStats.acc0Ms, 3) + "/" +
			                   formatFixed(debugStats.predictedTreeBuildMs, 3));
			hudLines.push_back(
			    "Debug step ms acc1/int/coll/merge/pub: " + formatFixed(debugStats.acc1Ms, 3) +
			    "/" + formatFixed(debugStats.integrateMs, 3) + "/" +
			    formatFixed(debugStats.collisionMs, 3) + "/" + formatFixed(debugStats.mergeMs, 3) +
			    "/" + formatFixed(debugStats.publishMs, 3));
		}
		if (!ui.statusMessage.empty()) {
			hudLines.push_back("Status: " + ui.statusMessage);
		}
		if (ui.creation.negativeMass() && ui.showNegativeMassWarning) {
			hudLines.push_back("Warning: Negative mass enabled (advanced mode).");
		}
		overlay.drawHudPanel(window, hudLines, ui.input.legendLines(ui.menu.active()), cfg.paused);

		if (ui.menu.active()) {
			const sf::Font* menuFont = overlay.fontPtr();
			if (menuFont != nullptr) {
				ui.menu.draw(window, *menuFont);
			}
		}

		window.display();
	}

	engine.stop();
	return 0;
}
