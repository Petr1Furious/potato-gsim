#include "net/MpClient.hpp"
#include "net/MpConstants.hpp"
#include "net/MpReplay.hpp"
#include "net/Protocol.hpp"
#include "render/Renderer.hpp"
#include "scenario/ScenarioManager.hpp"
#include "sim/SimulationEngine.hpp"
#include "ui/CreationPauseStateMachine.hpp"
#include "ui/InspectorOverlay.hpp"
#include "ui/QuantityFormat.hpp"
#include "ui/UiState.hpp"

#include <SFML/Graphics.hpp>

#include <enet/enet.h>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <iomanip>
#include <limits>
#include <map>
#include <optional>
#include <sstream>
#include <string>
#include <string_view>
#include <unordered_map>
#include <unordered_set>
#include <utility>
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

std::string previewName(const std::string& name) {
	if (name.empty()) {
		return "<none>";
	}
	constexpr std::size_t kPreviewLimit = 28;
	if (name.size() <= kPreviewLimit) {
		return name;
	}
	return name.substr(0, kPreviewLimit - 3) + "...";
}

bool isAsciiPrintable(std::uint32_t unicode) {
	return unicode >= 32 && unicode <= 126;
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
	const double cx = (minX + maxX) * 0.5;
	const double cy = (minY + maxY) * 0.5;
	const float sx = static_cast<float>(std::max(1.0, (maxX - minX) * 1.25));
	const float sy = static_cast<float>(std::max(1.0, (maxY - minY) * 1.25));
	renderer.setWorldOriginForRendering(cx, cy);
	renderer.setCameraWorldCenterDouble(cx, cy);
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

struct MpShipReplica {
	float facing = 0.f;
	std::uint8_t thrustForward = 0;
	std::uint8_t thrustReverse = 0;
	std::uint64_t lastTick = 0;
};

void collectReplicaThrustSamples(const std::unordered_map<sim::BodyId, MpShipReplica>& replicas,
                                 std::vector<net::ShipThrustSample>& out) {
	out.clear();
	for (const auto& [id, rep] : replicas) {
		const double f = static_cast<double>(rep.facing);
		const double ca = std::cos(f);
		const double sa = std::sin(f);
		double ax = 0.0;
		double ay = 0.0;
		if (rep.thrustForward) {
			ax += net::kShipThrustAccel * ca;
			ay += net::kShipThrustAccel * sa;
		}
		if (rep.thrustReverse) {
			ax -= net::kShipThrustAccel * ca * 0.5;
			ay -= net::kShipThrustAccel * sa * 0.5;
		}
		out.push_back(net::ShipThrustSample{.id = id, .ax = ax, .ay = ay});
	}
}

bool rewindReplayAuthority(net::MpReplayBuffer& replay,
                           sim::SimulationEngine& engine,
                           const std::uint64_t S,
                           const std::uint64_t H,
                           const sim::SimulationConfig& physicsCfg,
                           std::vector<sim::BodyDynamicsPatch> patchesAtS) {
	if (S >= H) {
		return false;
	}
	std::vector<std::pair<std::uint64_t, std::vector<net::ShipThrustSample>>> thrustsByStep;
	if (!replay.extractThrustsBetween(S, H, thrustsByStep)) {
		std::fprintf(stderr, "potato_gsim: MP rewind missing thrust history (S=%llu H=%llu)\n",
		             static_cast<unsigned long long>(S), static_cast<unsigned long long>(H));
		return false;
	}
	replay.popFramesAfter(S);
	net::ReplayFrame atS{};
	if (!replay.findFrame(S, atS)) {
		std::fprintf(stderr, "potato_gsim: MP rewind missing checkpoint at S=%llu\n",
		             static_cast<unsigned long long>(S));
		return false;
	}
	engine.queueApplyAuthoritativeSnapshot(std::move(atS.bodiesAfter));
	if (!patchesAtS.empty()) {
		engine.queuePatchBodyDynamics(std::move(patchesAtS));
	}
	const double ts = (std::isfinite(physicsCfg.timeScale) && physicsCfg.timeScale > 0.0)
	                      ? physicsCfg.timeScale
	                      : 1.0;
	const double dtSim = net::simulationDtFromTimeScale(ts);
	for (const auto& pr : thrustsByStep) {
		const std::uint64_t g = pr.first;
		net::applyShipThrustSamples(engine, pr.second);
		engine.advanceFixedStep(dtSim, physicsCfg);
		std::vector<sim::AuthoritativeBody> bodiesAfter;
		net::copyBodiesToAuthoritative(engine, bodiesAfter);
		replay.recordAfterPhysicsStep(g, std::move(bodiesAfter),
		                              std::vector<net::ShipThrustSample>(pr.second));
	}
	return true;
}

void catchUpPhysicsHead(net::MpReplayBuffer& replay,
                        sim::SimulationEngine& engine,
                        std::uint64_t steps,
                        const sim::SimulationConfig& physicsCfg,
                        const std::unordered_map<sim::BodyId, MpShipReplica>& replicas,
                        std::uint64_t& headRef) {
	if (steps == 0) {
		return;
	}
	const double ts = (std::isfinite(physicsCfg.timeScale) && physicsCfg.timeScale > 0.0)
	                      ? physicsCfg.timeScale
	                      : 1.0;
	const double dtSim = net::simulationDtFromTimeScale(ts);
	for (std::uint64_t k = 0; k < steps; ++k) {
		std::vector<net::ShipThrustSample> thrust;
		collectReplicaThrustSamples(replicas, thrust);
		net::applyShipThrustSamples(engine, thrust);
		engine.advanceFixedStep(dtSim, physicsCfg);
		++headRef;
		std::vector<sim::AuthoritativeBody> bodiesAfter;
		net::copyBodiesToAuthoritative(engine, bodiesAfter);
		replay.recordAfterPhysicsStep(headRef, std::move(bodiesAfter),
		                              std::vector<net::ShipThrustSample>(thrust));
	}
}

}  // namespace

int main(int argc, char** argv) {
	bool multiplayer = false;
	const char* mpHost = "127.0.0.1";
	std::uint16_t mpPort = 27777;
	if (argc >= 4 && std::string_view(argv[1]) == "--connect") {
		multiplayer = true;
		mpHost = argv[2];
		mpPort = static_cast<std::uint16_t>(std::strtoul(argv[3], nullptr, 10));
	}

	sf::RenderWindow window(sf::VideoMode({1400, 900}), "potato_gsim", sf::Style::Default,
	                        sf::State::Windowed);
	window.setVerticalSyncEnabled(true);
	window.setFramerateLimit(0);

	sim::SimulationConfig config;
	config.timeScale = 3600.0;
	config.fixedDtSeconds = net::kRealSecondsPerPhysicsStep;
	config.gravitationalConstant = 6.67430e-11;
	config.softeningEpsilon = 1.0e6;
	config.barnesHutTheta = 0.6;
	config.collisionCellScale = 8.0;
	config.collisionStepInterval = 1;
	config.workerCount = 0;
	config.parallelChunkSize = 0;
	config.chunkPolicy = sim::ChunkPolicy::DynamicClaim;
	config.directSerialMaxBodies = 224;
	config.directParallelMaxBodies = 768;
	if (multiplayer) {
		// Default until JoinAccept; matches server default (1 sim s / real s).
		config.timeScale = 1.0;
	}

	sim::SimulationEngine engine(config);

	std::optional<net::MpClient> mpClient;
	std::uint64_t mpHudServerTick = 0;
	sim::BodyId mpOwnShipId = 0;
	bool mpSessionJoined = false;
	bool mpJoinRequestSent = false;
	std::uint32_t mpInputSeq = 0;
	double mpWallPhysicsDebt = 0.0;
	std::uint64_t mpClientPhysicsHead = 0;
	std::uint64_t mpLastConfirmedAuthorityStep = 0;
	net::MpReplayBuffer mpReplay;
	bool mpPendingWorldHad = false;
	std::uint64_t mpPendingWorldTick = 0;
	std::uint64_t mpPendingWorldGlobalStep = 0;
	std::vector<sim::BodyId> mpPendingWorldIds;
	std::vector<double> mpPendingWorldPx;
	std::vector<double> mpPendingWorldPy;
	std::vector<double> mpPendingWorldVx;
	std::vector<double> mpPendingWorldVy;
	std::vector<net::MpClient::ShipNetSample> mpInboundShips;
	std::unordered_map<sim::BodyId, MpShipReplica> mpShipReplica;

	if (multiplayer) {
		if (enet_initialize() != 0) {
			return 1;
		}
		mpClient.emplace();
		if (!mpClient->connect(std::string(mpHost), mpPort)) {
			std::fprintf(stderr, "potato_gsim: could not connect to %s:%u\n", mpHost,
			             static_cast<unsigned>(mpPort));
			enet_deinitialize();
			return 1;
		}
	} else {
		engine.start();
	}

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
	    .maxAttractors = 320,
	    .predictionRecalcIntervalSeconds = 0.0,
	});
	engine.setDebugMetricsEnabled(ui.showDebugInfo);
	std::string pendingCreationName;

	auto applyScenario = [&](const std::vector<sim::SpawnCommand>& bodies) {
		engine.queueReplaceWorld(bodies);
		engine.resetSimulationTimer();
		ui.selection.clear();
		ui.traces.clear();
		ui.creation.cancel();
		pendingCreationName.clear();
		fitViewToBodies(renderer, bodies);
	};
	if (!multiplayer) {
		applyScenario(scenario::ScenarioManager::makePreset(ui.presetIndex));
	}

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
	std::optional<std::pair<double, double>> followViewOffset;
	bool wasFollowCamera = false;
	enum class NamePromptTarget { NextCreation, RenameSelected };
	struct NamePromptState {
		bool active = false;
		NamePromptTarget target = NamePromptTarget::NextCreation;
		std::optional<sim::BodyId> targetId;
		std::string buffer;
	};
	NamePromptState namePrompt;

	auto statusUntil = std::chrono::steady_clock::now();
	auto setStatus = [&](const std::string& message, double seconds) {
		ui.statusMessage = message;
		statusUntil = std::chrono::steady_clock::now() +
		              std::chrono::milliseconds(static_cast<int>(seconds * 1000.0));
	};
	auto beginNamePromptForNextCreation = [&]() {
		namePrompt.active = true;
		namePrompt.target = NamePromptTarget::NextCreation;
		namePrompt.targetId.reset();
		namePrompt.buffer = pendingCreationName;
		setStatus("Type name, Enter to apply, Esc to cancel.", 3.0);
	};
	auto beginNamePromptForSelectedRename = [&]() {
		if (!selectedBodyForInput.has_value()) {
			setStatus("Select a body first.", 2.0);
			return;
		}
		namePrompt.active = true;
		namePrompt.target = NamePromptTarget::RenameSelected;
		namePrompt.targetId = selectedBodyForInput->id;
		namePrompt.buffer = selectedBodyForInput->name;
		setStatus("Rename selected body: Enter to apply.", 3.0);
	};
	auto cancelNamePrompt = [&]() {
		namePrompt.active = false;
		namePrompt.targetId.reset();
		namePrompt.buffer.clear();
	};
	auto commitNamePrompt = [&]() {
		if (!namePrompt.active) {
			return;
		}
		if (namePrompt.target == NamePromptTarget::NextCreation) {
			pendingCreationName = namePrompt.buffer;
			setStatus("Next creation name set to " + previewName(pendingCreationName), 2.5);
		} else if (namePrompt.target == NamePromptTarget::RenameSelected &&
		           namePrompt.targetId.has_value()) {
			engine.queueRename(*namePrompt.targetId, namePrompt.buffer);
			setStatus(namePrompt.buffer.empty()
			              ? "Body name cleared."
			              : ("Body renamed to " + previewName(namePrompt.buffer)),
			          2.5);
		}
		cancelNamePrompt();
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
			case ui::Action::ToggleSelectedPrediction:
				ui.showSelectedBodyPrediction = !ui.showSelectedBodyPrediction;
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
					    .name = b.name,
					});
				}
				engine.queueReplaceWorld(transformed);
				ui.selection.clear();
				ui.traces.clear();
			} break;
			case ui::Action::RandomScenario:
				if (multiplayer) {
					break;
				}
				applyScenario(scenario::ScenarioManager::makeRandom(100000, 0.0, 0.0, 1e11));
				break;
			case ui::Action::ClearScenario:
				if (multiplayer) {
					break;
				}
				engine.queueClearAll();
				ui.selection.clear();
				ui.traces.clear();
				break;
			case ui::Action::NextPreset:
				if (multiplayer) {
					break;
				}
				if (!ui.presetNames.empty()) {
					ui.presetIndex = (ui.presetIndex + 1) % ui.presetNames.size();
					applyScenario(scenario::ScenarioManager::makePreset(ui.presetIndex));
				}
				break;
			case ui::Action::PrevPreset:
				if (multiplayer) {
					break;
				}
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
		} else if (action.itemId == "sim.timescale" && action.adjustDelta != 0) {
			if (multiplayer) {
				setStatus(
				    "Multiplayer: time scale is set on the server (potato_gsim_server [port] "
				    "[timeScale]).",
				    4.0);
			} else {
				engine.scaleTimeBy(action.adjustDelta > 0 ? 1.2 : (1.0 / 1.2));
			}
		} else if (action.itemId == "sim.theta" && action.adjustDelta != 0) {
			engine.setTheta(cfg.barnesHutTheta + action.adjustDelta * 0.03);
		} else if (action.itemId == "sim.softening" && action.adjustDelta != 0) {
			engine.setSoftening(cfg.softeningEpsilon * (action.adjustDelta > 0 ? 1.1 : 0.9));
		} else if (action.itemId == "sim.gravity" && action.adjustDelta != 0) {
			engine.setGravityConstant(cfg.gravitationalConstant *
			                          (action.adjustDelta > 0 ? 1.1 : 0.9));
		} else if (action.itemId == "sim.direct_serial_max" && action.adjustDelta != 0) {
			const std::size_t delta = action.adjustDelta > 0 ? 8u : 4u;
			const std::size_t current = cfg.directSerialMaxBodies;
			const std::size_t next = action.adjustDelta > 0
			                             ? (current + delta)
			                             : (current > delta ? current - delta : 0u);
			engine.setDirectSerialMaxBodies(next);
		} else if (action.itemId == "sim.direct_parallel_max" && action.adjustDelta != 0) {
			const std::size_t delta = action.adjustDelta > 0 ? 256u : 128u;
			const std::size_t current = cfg.directParallelMaxBodies;
			const std::size_t next = action.adjustDelta > 0
			                             ? (current + delta)
			                             : (current > delta ? current - delta : 0u);
			engine.setDirectParallelMaxBodies(next);
		} else if (action.itemId == "sim.collision_interval" && action.adjustDelta != 0) {
			const std::size_t next =
			    action.adjustDelta > 0
			        ? (cfg.collisionStepInterval + 1u)
			        : (cfg.collisionStepInterval > 1 ? cfg.collisionStepInterval - 1u : 1u);
			engine.setCollisionStepInterval(next);
		} else if (action.itemId == "sim.chunk_size" && action.adjustDelta != 0) {
			const std::size_t current = cfg.parallelChunkSize;
			if (action.adjustDelta > 0) {
				engine.setParallelChunkSize(current == 0 ? 32u : (current + 32u));
			} else if (current == 0) {
				engine.setParallelChunkSize(0);
			} else if (current <= 32u) {
				engine.setParallelChunkSize(0);
			} else {
				engine.setParallelChunkSize(current - 32u);
			}
		} else if (action.itemId == "sim.chunk_policy") {
			engine.setChunkPolicy(cfg.chunkPolicy == sim::ChunkPolicy::DynamicClaim
			                          ? sim::ChunkPolicy::StaticCyclic
			                          : sim::ChunkPolicy::DynamicClaim);
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
		} else if (action.itemId == "creation.next_name" && action.activate) {
			beginNamePromptForNextCreation();
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
		} else if (action.itemId == "predict.selected") {
			ui.showSelectedBodyPrediction = !ui.showSelectedBodyPrediction;
		} else if (action.itemId == "labels.toggle") {
			ui.showHoverLabels = !ui.showHoverLabels;
		} else if (action.itemId == "labels.always") {
			ui.showAlwaysNames = !ui.showAlwaysNames;
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
		} else if (action.itemId == "predict.interval" && action.adjustDelta != 0) {
			auto predictorSettings = ui.predictor.settings();
			const double step =
			    predictorSettings.predictionRecalcIntervalSeconds < 0.1 ? 0.01 : 0.05;
			double next =
			    predictorSettings.predictionRecalcIntervalSeconds + action.adjustDelta * step;
			if (next < 0.005) {
				next = 0.0;
			}
			predictorSettings.predictionRecalcIntervalSeconds = std::clamp(next, 0.0, 2.0);
			ui.predictor.setSettings(predictorSettings);
		} else if (action.itemId == "action.random") {
			if (!multiplayer) {
				dispatchAction(ui::Action::RandomScenario);
			}
		} else if (action.itemId == "action.clear") {
			if (!multiplayer) {
				dispatchAction(ui::Action::ClearScenario);
			}
		} else if (action.itemId == "action.prevpreset") {
			if (!multiplayer) {
				dispatchAction(ui::Action::PrevPreset);
			}
		} else if (action.itemId == "action.nextpreset") {
			if (!multiplayer) {
				dispatchAction(ui::Action::NextPreset);
			}
		} else if (action.itemId == "action.resettimer") {
			dispatchAction(ui::Action::ResetTimer);
		} else if (action.itemId == "action.resetframe") {
			dispatchAction(ui::Action::AdvancedResetFrame);
		} else if (action.itemId == "action.rename_selected" && action.activate) {
			beginNamePromptForSelectedRename();
		}
	};

	auto lastFrameTime = std::chrono::steady_clock::now();
	auto lastPredictTime = lastFrameTime;
	double fps = 0.0;
	const auto currentSimSecondsPerRealSecond = [&engine]() {
		const sim::SimulationConfig liveCfg = engine.config();
		if (!liveCfg.paused) {
			const double measured = engine.simulatedSecondsPerRealSecond();
			if (std::isfinite(measured) && measured > 0.0) {
				return measured;
			}
		}
		if (!std::isfinite(liveCfg.timeScale) || liveCfg.timeScale <= 0.0) {
			return 1.0;
		}
		return liveCfg.timeScale;
	};
	const auto panView = [&](const sf::Vector2i& pixelDelta) {
		const sf::Vector2f before = renderer.view().getCenter();
		renderer.panByPixels(pixelDelta);
		if (ui.selection.followEnabled() && followViewOffset.has_value()) {
			const sf::Vector2f after = renderer.view().getCenter();
			followViewOffset->first += static_cast<double>(after.x - before.x);
			followViewOffset->second += static_cast<double>(after.y - before.y);
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
				const render::Renderer::WorldCoordsD mw = renderer.screenToWorldD(moved->position);
				ui.creation.updateCursor(
				    sf::Vector2f(static_cast<float>(mw.x), static_cast<float>(mw.y)));
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

				const render::Renderer::WorldCoordsD wd =
				    renderer.screenToWorldD(mousePressed->position);
				const sf::Vector2f world(static_cast<float>(wd.x), static_cast<float>(wd.y));
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
							if (multiplayer) {
								setStatus("Creation is disabled in multiplayer.", 2.0);
							} else {
								sim::SpawnCommand namedSpawn = *spawn;
								namedSpawn.name = pendingCreationName;
								engine.queueSpawn(namedSpawn);
								pendingCreationName.clear();
								if (ui.creation.negativeMass() && ui.showNegativeMassWarning) {
									setStatus("Negative-mass spawn created (advanced mode).", 3.0);
								}
							}
						}
					} else {
						leftPanning = true;
						leftDragMaySelect = true;
						leftPanPixel = mousePressed->position;
						leftDownPixel = mousePressed->position;
					}
				} else if (mousePressed->button == sf::Mouse::Button::Right) {
					if (!multiplayer) {
						const double pickRadius =
						    std::max(1.0f, renderer.worldUnitsPerPixel() * 20.0f);
						engine.queueDeleteNearest(world.x, world.y, pickRadius);
					}
				}
			}
			if (const auto* mouseReleased = event->getIf<sf::Event::MouseButtonReleased>()) {
				if (mouseReleased->button == sf::Mouse::Button::Middle) {
					middlePanning = false;
				}
				if (mouseReleased->button == sf::Mouse::Button::Left) {
					if (leftPanning && leftDragMaySelect && !ui.menu.active() &&
					    !ui.creation.enabled()) {
						const render::Renderer::WorldCoordsD wrel =
						    renderer.screenToWorldD(mouseReleased->position);
						const sf::Vector2f world(static_cast<float>(wrel.x),
						                         static_cast<float>(wrel.y));
						const auto worldToScreenDistance = [&](const sf::Vector2f& delta) -> float {
							const sf::Vector2i p0 = renderer.worldToPixelD(wrel.x, wrel.y);
							const sf::Vector2i p1 =
							    renderer.worldToPixelD(wrel.x + static_cast<double>(delta.x),
							                           wrel.y + static_cast<double>(delta.y));
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
			if (const auto* textEntered = event->getIf<sf::Event::TextEntered>()) {
				if (!namePrompt.active) {
					continue;
				}
				if (isAsciiPrintable(textEntered->unicode) && namePrompt.buffer.size() < 48) {
					namePrompt.buffer.push_back(static_cast<char>(textEntered->unicode));
				}
				continue;
			}
			if (const auto* key = event->getIf<sf::Event::KeyPressed>()) {
				if (namePrompt.active) {
					if (key->code == sf::Keyboard::Key::Backspace) {
						if (!namePrompt.buffer.empty()) {
							namePrompt.buffer.pop_back();
						}
					} else if (key->code == sf::Keyboard::Key::Enter) {
						commitNamePrompt();
					} else if (key->code == sf::Keyboard::Key::Escape) {
						cancelNamePrompt();
					}
					continue;
				}
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

		const ui::HoldAdjustments holds =
		    ui.input.computeHolds(frameDt, !ui.menu.active(), !multiplayer);
		if (holds.timeScaleFactor != 1.0 && !multiplayer) {
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
			followViewOffset->first +=
			    static_cast<double>(viewCenterAfterUpdate.x - viewCenterBeforeUpdate.x);
			followViewOffset->second +=
			    static_cast<double>(viewCenterAfterUpdate.y - viewCenterBeforeUpdate.y);
		}

		if (multiplayer && mpClient.has_value()) {
			mpClient->service(0);
			if (!mpJoinRequestSent && mpClient->isPeerConnected()) {
				mpClient->sendJoinRequest();
				mpJoinRequestSent = true;
			}
			std::uint64_t joinTick = 0;
			std::uint64_t joinGlobalPhysicsStep = 0;
			std::vector<sim::AuthoritativeBody> joinBodies;
			sim::BodyId joinOwn = 0;
			double joinTimeScale = 1.0;
			mpClient->takeJoinAccept(joinTick, joinGlobalPhysicsStep, joinBodies, joinOwn,
			                         joinTimeScale);
			if (!joinBodies.empty()) {
				std::vector<sim::AuthoritativeBody> joinReplaySeed = joinBodies;
				engine.queueApplyAuthoritativeSnapshot(std::move(joinBodies));
				engine.setTimeScale(joinTimeScale);
				mpOwnShipId = joinOwn;
				mpSessionJoined = true;
				if (mpOwnShipId != 0) {
					ui.selection.setSelected(mpOwnShipId);
				}
				engine.copyBodies(bodies);
				std::vector<sim::SpawnCommand> fitBodies;
				fitBodies.reserve(bodies.size());
				for (const sim::BodySnapshot& b : bodies) {
					fitBodies.push_back(sim::SpawnCommand{
					    .x = b.x,
					    .y = b.y,
					    .vx = b.vx,
					    .vy = b.vy,
					    .mass = b.mass,
					    .radius = b.radius,
					    .name = b.name,
					});
				}
				fitViewToBodies(renderer, fitBodies);
				mpHudServerTick = joinTick;
				mpWallPhysicsDebt = 0.0;
				mpClientPhysicsHead = joinGlobalPhysicsStep;
				mpLastConfirmedAuthorityStep = joinGlobalPhysicsStep;
				mpReplay.seedAfterJoin(joinGlobalPhysicsStep, std::move(joinReplaySeed));
				setStatus("Joined multiplayer session.", 2.5);
			}
			if (mpSessionJoined) {
				bool hadMerge = false;
				std::uint64_t mergeTick = 0;
				std::vector<std::pair<sim::BodyId, sim::BodyId>> netMerges;
				mpClient->takeMergeRemaps(mergeTick, netMerges, hadMerge);
				if (hadMerge) {
					for (const auto& pr : netMerges) {
						engine.queueDelete(pr.first);
					}
					ui.selection.applyMergeRemap(netMerges);
					ui.traces.applyMergeRemap(netMerges);
					mpReplay.clear();
				}

				mpClient->takeShipSamples(mpInboundShips);
				for (const net::MpClient::ShipNetSample& s : mpInboundShips) {
					MpShipReplica& rep = mpShipReplica[s.bodyId];
					rep.facing = s.facingRadians;
					rep.thrustForward = s.thrustForward;
					rep.thrustReverse = s.thrustReverse;
					rep.lastTick = s.serverTick;
				}

				mpPendingWorldHad = false;
				mpClient->takeWorldSnapshot(mpPendingWorldTick, mpPendingWorldGlobalStep,
				                            mpPendingWorldIds, mpPendingWorldPx, mpPendingWorldPy,
				                            mpPendingWorldVx, mpPendingWorldVy, mpPendingWorldHad);

				if (mpOwnShipId != 0) {
					double facing = 0.0;
					if (const std::optional<sim::BodySnapshot> self =
					        engine.bodyById(mpOwnShipId)) {
						const render::Renderer::WorldCoordsD mouseWorld =
						    renderer.screenToWorldD(sf::Mouse::getPosition(window));
						facing = std::atan2(mouseWorld.y - self->y, mouseWorld.x - self->x);
					} else if (const auto it = mpShipReplica.find(mpOwnShipId);
					           it != mpShipReplica.end()) {
						facing = static_cast<double>(it->second.facing);
					}
					net::ClientInputPayload in{};
					in.seq = ++mpInputSeq;
					in.thrustForward = static_cast<std::uint8_t>(
					    sf::Keyboard::isKeyPressed(sf::Keyboard::Key::W) ||
					            sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Up)
					        ? 1
					        : 0);
					in.thrustReverse = static_cast<std::uint8_t>(
					    sf::Keyboard::isKeyPressed(sf::Keyboard::Key::S) ||
					            sf::Keyboard::isKeyPressed(sf::Keyboard::Key::Down)
					        ? 1
					        : 0);
					in.facingRadians = static_cast<float>(facing);
					mpClient->sendInput(in);
				}
			}
		}

		if (multiplayer && mpSessionJoined) {
			const sim::SimulationConfig physicsCfg = engine.config();
			if (!physicsCfg.paused) {
				const double ts =
				    (std::isfinite(physicsCfg.timeScale) && physicsCfg.timeScale > 0.0)
				        ? physicsCfg.timeScale
				        : 1.0;
				const double dtSim = net::simulationDtFromTimeScale(ts);

				for (const net::MpClient::ShipNetSample& s : mpInboundShips) {
					mpHudServerTick = std::max(mpHudServerTick, s.serverTick);
				}
				if (mpPendingWorldHad) {
					mpHudServerTick = std::max(mpHudServerTick, mpPendingWorldTick);
				}

				std::unordered_set<sim::BodyId> shipDynamicsFromNet;
				for (const net::MpClient::ShipNetSample& s : mpInboundShips) {
					shipDynamicsFromNet.insert(s.bodyId);
				}
				for (const auto& idRep : mpShipReplica) {
					shipDynamicsFromNet.insert(idRep.first);
				}
				if (mpOwnShipId != 0) {
					shipDynamicsFromNet.insert(mpOwnShipId);
				}

				std::map<std::uint64_t, std::vector<sim::BodyDynamicsPatch>> stepPatches;
				if (mpPendingWorldHad) {
					auto& into = stepPatches[mpPendingWorldGlobalStep];
					for (std::size_t i = 0; i < mpPendingWorldIds.size(); ++i) {
						if (shipDynamicsFromNet.count(mpPendingWorldIds[i]) != 0) {
							continue;
						}
						into.push_back(sim::BodyDynamicsPatch{
						    .id = mpPendingWorldIds[i],
						    .x = mpPendingWorldPx[i],
						    .y = mpPendingWorldPy[i],
						    .vx = mpPendingWorldVx[i],
						    .vy = mpPendingWorldVy[i],
						});
					}
				}
				for (const net::MpClient::ShipNetSample& s : mpInboundShips) {
					stepPatches[s.globalPhysicsStep].push_back(sim::BodyDynamicsPatch{
					    .id = s.bodyId,
					    .x = s.px,
					    .y = s.py,
					    .vx = s.vx,
					    .vy = s.vy,
					});
				}

				std::vector<std::uint64_t> stepKeys;
				stepKeys.reserve(stepPatches.size());
				for (const auto& pr : stepPatches) {
					stepKeys.push_back(pr.first);
				}
				std::sort(stepKeys.begin(), stepKeys.end());
				for (const std::uint64_t S : stepKeys) {
					const auto itSp = stepPatches.find(S);
					if (itSp == stepPatches.end()) {
						continue;
					}
					std::vector<sim::BodyDynamicsPatch> patches = std::move(itSp->second);
					const std::uint64_t H = mpClientPhysicsHead;
					if (S < H) {
						if (!rewindReplayAuthority(mpReplay, engine, S, H, physicsCfg,
						                           std::move(patches))) {
							std::fprintf(
							    stderr,
							    "potato_gsim: MP replay horizon exceeded; resyncing buffer\n");
							std::vector<sim::AuthoritativeBody> snap;
							net::copyBodiesToAuthoritative(engine, snap);
							mpReplay.clear();
							mpReplay.seedAfterJoin(mpClientPhysicsHead, std::move(snap));
						} else {
							mpClientPhysicsHead = H;
						}
					} else if (S > H) {
						catchUpPhysicsHead(mpReplay, engine, S - H, physicsCfg, mpShipReplica,
						                   mpClientPhysicsHead);
						if (!patches.empty()) {
							engine.queuePatchBodyDynamics(std::move(patches));
						}
					} else {
						if (!patches.empty()) {
							engine.queuePatchBodyDynamics(std::move(patches));
						}
					}
					mpLastConfirmedAuthorityStep = std::max(mpLastConfirmedAuthorityStep, S);
				}

				mpWallPhysicsDebt += frameDt;
				mpWallPhysicsDebt = std::min(mpWallPhysicsDebt, net::kMaxWallPhysicsDebtSeconds);
				int stepBudget = 0;
				while (mpWallPhysicsDebt >= net::kRealSecondsPerPhysicsStep &&
				       stepBudget < net::kMaxCatchUpPhysicsStepsPerFrame) {
					if (mpClientPhysicsHead >=
					    mpLastConfirmedAuthorityStep + net::kMaxPredictionLeadPhysicsSteps) {
						break;
					}
					std::vector<net::ShipThrustSample> thrust;
					collectReplicaThrustSamples(mpShipReplica, thrust);
					net::applyShipThrustSamples(engine, thrust);
					engine.advanceFixedStep(dtSim, physicsCfg);
					++mpClientPhysicsHead;
					std::vector<sim::AuthoritativeBody> bodiesAfter;
					net::copyBodiesToAuthoritative(engine, bodiesAfter);
					mpReplay.recordAfterPhysicsStep(mpClientPhysicsHead, std::move(bodiesAfter),
					                                std::vector<net::ShipThrustSample>(thrust));
					mpWallPhysicsDebt -= net::kRealSecondsPerPhysicsStep;
					++stepBudget;
				}
			}
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
		const bool followCamNow = ui.selection.followEnabled() && selectedBody.has_value();
		if (followCamNow) {
			renderer.setWorldOriginForRendering(selectedBody->x, selectedBody->y);
			const double sx = selectedBody->x;
			const double sy = selectedBody->y;
			if (!trackedFollowId.has_value() || *trackedFollowId != selectedBody->id ||
			    !followViewOffset.has_value()) {
				const render::Renderer::WorldCoordsD cc = renderer.cameraWorldCenter();
				followViewOffset = {cc.x - sx, cc.y - sy};
			}
			renderer.setCameraWorldCenterDouble(sx + followViewOffset->first,
			                                    sy + followViewOffset->second);
			trackedFollowId = selectedBody->id;
		} else {
			if (wasFollowCamera) {
				renderer.clearWorldRenderingOrigin();
			}
			trackedFollowId.reset();
			followViewOffset.reset();
		}
		wasFollowCamera = followCamNow;
		const std::optional<sim::BodySnapshot> selectedForCreation =
		    (selectedBody.has_value() &&
		     (ui.creation.relativeFrame() || ui.selection.followEnabled()))
		        ? selectedBody
		        : std::nullopt;
		if (const std::optional<sf::Vector2i> autoStartPixel =
		        creationPauseState.consumeAutoStartPixel();
		    autoStartPixel.has_value() && !ui.creation.isInProgress()) {
			const render::Renderer::WorldCoordsD sw = renderer.screenToWorldD(*autoStartPixel);
			const sf::Vector2f startWorld(static_cast<float>(sw.x), static_cast<float>(sw.y));
			(void)ui.creation.handleLeftClick(startWorld, selectedForCreation,
			                                  currentSimSecondsPerRealSecond());
		}
		{
			const render::Renderer::WorldCoordsD cw =
			    renderer.screenToWorldD(sf::Mouse::getPosition(window));
			ui.creation.updateCursor(
			    sf::Vector2f(static_cast<float>(cw.x), static_cast<float>(cw.y)),
			    selectedForCreation);
		}
		selectedBodyForInput = selectedBody;
		ui.traces.ingest(bodies, engine.simulationTimeSeconds());
		const render::Renderer::WorldCoordsD cursorWorldD =
		    renderer.screenToWorldD(sf::Mouse::getPosition(window));
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
			const double dx = b.x - cursorWorldD.x;
			const double dy = b.y - cursorWorldD.y;
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
		predictorSettings.predictionRecalcIntervalSeconds =
		    std::max(0.0, predictorSettings.predictionRecalcIntervalSeconds);
		const double predictWindowSimSeconds =
		    predictorSettings.realTimeHorizonSeconds * displayTimeRate;
		predictorSettings.dt =
		    std::max(1e-9, predictWindowSimSeconds / static_cast<double>(predictorSettings.steps));
		ui.predictor.setSettings(predictorSettings);
		const double simSecondsPerUpdate =
		    (engine.simulatedSecondsPerUpdate() > 0.0)
		        ? engine.simulatedSecondsPerUpdate()
		        : ((std::isfinite(cfg.timeScale) && cfg.timeScale > 0.0) ? cfg.timeScale : 1.0);
		const std::string timeScaleDisplay = ui::formatTimeLegacy(displayTimeRate) + "/s (" +
		                                     ui::formatTimeLegacy(simSecondsPerUpdate) + "/U)";
		const std::string predictWindowDisplay =
		    formatFixed(predictorSettings.realTimeHorizonSeconds, 2) + " s real (" +
		    ui::formatTimeLegacy(predictWindowSimSeconds) + " sim)";
		const std::string predictIntervalDisplay =
		    predictorSettings.predictionRecalcIntervalSeconds <= 0.0
		        ? "Every frame"
		        : (formatFixed(predictorSettings.predictionRecalcIntervalSeconds, 3) + " s");
		const std::string scaleDisplay =
		    ui::formatDistanceLegacy(renderer.worldUnitsPerPixel()) + "/pixel";
		const std::optional<sim::SpawnCommand> creationPreviewSpawn =
		    ui.creation.previewSpawn(creationReferenceBody, currentSimSecondsPerRealSecond());

		const double predictElapsedSeconds =
		    std::chrono::duration<double>(now - lastPredictTime).count();
		const bool shouldRefreshPrediction =
		    ui.showPredictions &&
		    (predictorSettings.predictionRecalcIntervalSeconds <= 0.0 ||
		     predictElapsedSeconds >= predictorSettings.predictionRecalcIntervalSeconds);
		if (shouldRefreshPrediction) {
			lastPredictTime = now;
			selectedPredictionOffsets.clear();
			selectedPredictionBodyId.reset();
			creationPredictionOffsets.clear();
			creationPredictionRelativeSelectedId.reset();
			if (ui.showSelectedBodyPrediction && selectedBody.has_value()) {
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
		if (ui.showPredictions && ui.showSelectedBodyPrediction && selectedBody.has_value() &&
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
		    {"sim.timescale", "Time Speed", timeScaleDisplay, true},
		    {"sim.theta", "Barnes-Hut Theta", formatFixed(cfg.barnesHutTheta), true},
		    {"sim.softening", "Softening Epsilon", ui::formatDistanceLegacy(cfg.softeningEpsilon),
		     true},
		    {"sim.gravity", "Gravity Constant (SI)", formatScientific(cfg.gravitationalConstant),
		     true},
		    {"sim.direct_serial_max", "Direct Serial Max Bodies",
		     std::to_string(cfg.directSerialMaxBodies), true},
		    {"sim.direct_parallel_max", "Direct Parallel Max Bodies",
		     std::to_string(cfg.directParallelMaxBodies), true},
		    {"sim.collision_interval", "Collision Step Interval",
		     std::to_string(cfg.collisionStepInterval), true},
		    {"sim.chunk_size", "Parallel Chunk Size (0=auto)",
		     std::to_string(cfg.parallelChunkSize), true},
		    {"sim.chunk_policy", "Chunk Policy",
		     cfg.chunkPolicy == sim::ChunkPolicy::DynamicClaim ? "Dynamic claim" : "Static cyclic",
		     false},
		    {"sim.workers", "Worker Count (0=auto)", std::to_string(cfg.workerCount), true},
		    {"creation.enabled", "Creation Tool", ui.creation.enabled() ? "On" : "Off", false},
		    {"creation.density", "Creation Density (kg/m^3)", formatFixed(ui.creation.density(), 1),
		     true},
		    {"creation.negmass", "Negative Mass (Advanced)",
		     ui.creation.negativeMass() ? "On" : "Off", false},
		    {"creation.relframe", "Creation Relative Frame",
		     ui.creation.relativeFrame() ? "Selected" : "World", false},
		    {"creation.next_name", "Creation Name (next body)", previewName(pendingCreationName),
		     false},
		    {"trace.enabled", "Trails", ui.traces.settings().enabled ? "On" : "Off", false},
		    {"trace.relative", "Relative Trails", ui.traces.settings().relative ? "On" : "Off",
		     false},
		    {"predict.enabled", "Predictions", ui.showPredictions ? "On" : "Off", false},
		    {"predict.selected", "Selected Body Prediction",
		     ui.showSelectedBodyPrediction ? "On" : "Off", false},
		    {"predict.interval", "Prediction Recalc Interval", predictIntervalDisplay, true},
		    {"labels.toggle", "Hover Labels", ui.showHoverLabels ? "On" : "Off", false},
		    {"labels.always", "Always Show Names", ui.showAlwaysNames ? "On" : "Off", false},
		    {"debug.toggle", "Debug Overlay", ui.showDebugInfo ? "On" : "Off", false},
		    {"predict.realtime_window", "Prediction Horizon (Real Time)", predictWindowDisplay,
		     true},
		    {"predict.steps", "Prediction Steps", std::to_string(predictorSettings.steps), true},
		    {"action.random", "Random Scenario", "F5", false},
		    {"action.clear", "Clear World", "F6", false},
		    {"action.prevpreset", "Previous Preset", "F7", false},
		    {"action.nextpreset", "Next Preset", "F8", false},
		    {"action.resettimer", "Reset Timer", "J", false},
		    {"action.resetframe", "Reset Frame of Reference", "Q", false},
		    {"action.rename_selected", "Rename Selected Body",
		     selectedBody.has_value() ? previewName(selectedBody->name)
		                              : std::string("Select body"),
		     false},
		};
		ui.menu.setItems(menuItems);

		renderer.draw(bodies);
		{
			double rox = 0.0;
			double roy = 0.0;
			bool useRo = false;
			renderer.worldRenderingOrigin(rox, roy, useRo);
			ui.traces.draw(window, selectedBody, rox, roy, useRo);
		}

		if (ui.creation.enabled() && ui.creation.isInProgress()) {
			sf::Vector2f anchor = ui.creation.anchor();
			const float radius = ui.creation.previewRadius();
			sf::Vector2f cursor = ui.creation.cursor();
			const sf::Vector2f anchorL = renderer.worldToRenderLocal(static_cast<double>(anchor.x),
			                                                         static_cast<double>(anchor.y));
			const sf::Vector2f cursorL = renderer.worldToRenderLocal(static_cast<double>(cursor.x),
			                                                         static_cast<double>(cursor.y));
			sf::CircleShape ghost(radius);
			ghost.setOrigin(sf::Vector2f(radius, radius));
			ghost.setPosition(anchorL);
			ghost.setFillColor(sf::Color(255, 255, 255, 35));
			ghost.setOutlineColor(sf::Color(230, 230, 255, 160));
			ghost.setOutlineThickness(std::max(0.04f, radius * 0.08f));
			window.draw(ghost);

			sf::VertexArray arrow(sf::PrimitiveType::Lines, 2);
			arrow[0].position = anchorL;
			arrow[1].position = cursorL;
			arrow[0].color = sf::Color(255, 170, 120, 210);
			arrow[1].color = sf::Color(255, 170, 120, 210);
			window.draw(arrow);

			if (ui.showPredictions && !creationPrediction.empty()) {
				sf::VertexArray strip(sf::PrimitiveType::LineStrip, creationPrediction.size());
				for (std::size_t i = 0; i < creationPrediction.size(); ++i) {
					strip[i].position =
					    renderer.worldToRenderLocal(static_cast<double>(creationPrediction[i].x),
					                                static_cast<double>(creationPrediction[i].y));
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

				const sf::Vector2i anchorPx = renderer.worldToPixelD(previewX, previewY);
				const sf::Vector2i radiusEdgePx =
				    renderer.worldToPixelD(previewX + static_cast<double>(radius), previewY);
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

		const auto worldToRenderLocal = [&](double x, double y) {
			return renderer.worldToRenderLocal(x, y);
		};
		const auto worldToPixel = [&](double x, double y) { return renderer.worldToPixelD(x, y); };
		overlay.drawWorldSelection(window, selectedBody, std::nullopt, selectedPrediction,
		                           std::nullopt, displayTimeRate, true, false, worldToRenderLocal,
		                           worldToPixel);
		if (ui.showHoverLabels) {
			overlay.drawWorldSelection(window, hoveredBody, selectedBody, {}, std::nullopt,
			                           displayTimeRate, false, true, worldToRenderLocal,
			                           worldToPixel);
		}
		if (ui.showAlwaysNames) {
			overlay.drawLabels(window, bodies, ui.selection.selectedId(), worldToPixel,
			                   std::numeric_limits<float>::max());
		}

		if (std::chrono::steady_clock::now() > statusUntil) {
			ui.statusMessage.clear();
		}
		std::vector<std::string> hudLines;
		if (multiplayer) {
			if (!mpSessionJoined) {
				hudLines.push_back("Multiplayer: connecting to " + std::string(mpHost) + ":" +
				                   std::to_string(static_cast<unsigned>(mpPort)) + "…");
			} else {
				const sim::SimulationConfig liveCfg = engine.config();
				const double ts = (std::isfinite(liveCfg.timeScale) && liveCfg.timeScale > 0.0)
				                      ? liveCfg.timeScale
				                      : 1.0;
				hudLines.push_back("Multiplayer: tick " + std::to_string(mpHudServerTick) +
				                   " | ship id " + std::to_string(mpOwnShipId) + " | sim speed " +
				                   ui::formatTimeLegacy(ts) +
				                   "/s real | 240 phys steps/s wall, Δt_sim=scale/240 (server: "
				                   "potato_gsim_server …)");
			}
		}
		hudLines.push_back("Bodies: " + std::to_string(engine.bodyCount()) + " | Preset: " +
		                   (ui.presetNames.empty()
		                        ? std::string("n/a")
		                        : ui.presetNames[ui.presetIndex % ui.presetNames.size()]));
		hudLines.push_back("Time speed: " + timeScaleDisplay +
		                   " | Timer: " + ui::formatTimeLegacy(engine.simulationTimeSeconds()) +
		                   " (" + formatScientific(engine.simulationTimeSeconds()) + " s)");
		hudLines.push_back("Scale: " + scaleDisplay + " | Predict: " + predictWindowDisplay +
		                   " | UPS: " + std::to_string(round3(engine.updatesPerSecond())) +
		                   " | FPS: " + std::to_string(round3(fps)));
		hudLines.push_back("Trails: " + std::string(ui.traces.settings().enabled ? "On" : "Off") +
		                   " | Predictions: " + std::string(ui.showPredictions ? "On" : "Off") +
		                   " | Labels: " + std::string(ui.showHoverLabels ? "On" : "Off"));
		if (ui.showDebugInfo) {
			hudLines.push_back("Debug chunks: " + std::to_string(debugStats.chunkSize) + " (" +
			                   std::string(debugStats.chunkPolicy) + ") | collision phase: " +
			                   std::string(debugStats.collisionPhaseExecuted ? "ran" : "skipped"));
			const double contrib0 = debugStats.avgContributionsAcc0;
			const double contrib1 = debugStats.avgContributionsAcc1;
			const bool isBarnesHut =
			    std::string(debugStats.forceAlgorithm) == "barnes_hut_parallel";
			const std::string treeLine =
			    isBarnesHut
			        ? ("Debug algo: " + std::string(debugStats.forceAlgorithm) +
			           " | tree nodes cur/pred: " + std::to_string(debugStats.currentTreeNodes) +
			           "/" + std::to_string(debugStats.predictedTreeNodes) +
			           " | avg contrib/body: " + formatFixed(contrib0, 1) + "/" +
			           formatFixed(contrib1, 1))
			        : ("Debug algo: " + std::string(debugStats.forceAlgorithm) +
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
		if (namePrompt.active) {
			const std::string target = namePrompt.target == NamePromptTarget::NextCreation
			                               ? "next creation"
			                               : "selected body";
			hudLines.push_back("Name input (" + target + "): " + namePrompt.buffer +
			                   "  [Enter apply | Esc cancel]");
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

	if (multiplayer) {
		mpClient.reset();
		enet_deinitialize();
	}
	engine.stop();
	return 0;
}
