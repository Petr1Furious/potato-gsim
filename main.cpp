#include "net/MpClient.hpp"
#include "net/MpClientSim.hpp"
#include "net/MpConstants.hpp"
#include "net/Protocol.hpp"
#include "render/Renderer.hpp"
#include "sim/SimulationEngine.hpp"
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
#include <optional>
#include <sstream>
#include <string>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <vector>

namespace {

double round3(double v) {
	return std::round(v * 1000.0) / 1000.0;
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

using MpShipReplica = net::MpShipReplicaInput;

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
	std::optional<net::MpClientSim> mpSim;
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
	ui.traces.setSettings(ui::TraceStore::Settings{
	    .enabled = false,
	    .relative = true,
	    .maxPointsPerBody = 260,
	});
	ui.predictor.setSettings(ui::Predictor::Settings{
	    .enabled = true,
	    .steps = 900,
	    .dt = 1.0 / 120.0,
	    .realTimeHorizonSeconds = 5.0,
	    .maxAttractors = 20,
	    .predictionRecalcIntervalSeconds = 0.0,
	});
	engine.setDebugMetricsEnabled(false);

	auto applyScenario = [&](const std::vector<sim::SpawnCommand>& spawnBodies) {
		std::vector<sim::SpawnCommand> bodies = spawnBodies;
		engine.queueReplaceWorld(bodies);
		engine.resetSimulationTimer();
		ui.selection.clear();
		ui.traces.clear();
		fitViewToBodies(renderer, bodies);
	};
	if (!multiplayer) {
		applyScenario({});
	}

	bool middlePanning = false;
	sf::Vector2i lastPanPixel{0, 0};
	bool leftPanning = false;
	bool leftDragMaySelect = false;
	sf::Vector2i leftPanPixel{0, 0};
	sf::Vector2i leftDownPixel{0, 0};
	bool fullscreen = false;
	std::vector<sim::BodySnapshot> bodies;
	std::vector<std::pair<sim::BodyId, sim::BodyId>> mergeRemapEvents;
	std::vector<sf::Vector2f> shipPrediction;
	std::vector<sf::Vector2f> shipPredictionOffsets;
	std::optional<sim::BodyId> shipPredictionBodyId;
	std::optional<sim::BodyId> trackedFollowId;
	std::optional<std::pair<double, double>> followViewOffset;
	bool wasFollowCamera = false;
	std::optional<net::MpClientRenderPublish> mpRenderFrame;

	auto statusUntil = std::chrono::steady_clock::now();
	auto setStatus = [&](const std::string& message, double seconds) {
		ui.statusMessage = message;
		statusUntil = std::chrono::steady_clock::now() +
		              std::chrono::milliseconds(static_cast<int>(seconds * 1000.0));
	};
	auto openMenu = [&](bool active) {
		if (ui.menu.active() == active) {
			return;
		}
		ui.menu.setActive(active);
	};

	auto dispatchAction = [&](ui::Action action) {
		switch (action) {
			case ui::Action::ToggleMenu:
				openMenu(!ui.menu.active());
				break;
			case ui::Action::ToggleFullscreen:
				fullscreen = !fullscreen;
				setWindowFullscreen(window, fullscreen);
				renderer.onResize(window.getSize());
				break;
			case ui::Action::ResetView:
				renderer.resetView();
				break;
			case ui::Action::ToggleFollowMode:
				ui.followCameraMode =
				    (ui.followCameraMode == ui::UiState::FollowCameraMode::FollowOwnShip)
				        ? ui::UiState::FollowCameraMode::FollowSelectionOrOrigin
				        : ui::UiState::FollowCameraMode::FollowOwnShip;
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
			case ui::Action::ToggleShipPrediction:
				ui.showShipSelfPrediction = !ui.showShipSelfPrediction;
				break;
		}
	};

	auto dispatchMenuAction = [&](const ui::MenuAction& action) {
		if (action.itemId == "trace.enabled") {
			auto s = ui.traces.settings();
			s.enabled = !s.enabled;
			ui.traces.setSettings(s);
			if (!s.enabled) {
				ui.traces.clear();
			}
		} else if (action.itemId == "predict.ship") {
			ui.showShipSelfPrediction = !ui.showShipSelfPrediction;
		} else if (action.itemId == "follow.mode") {
			dispatchAction(ui::Action::ToggleFollowMode);
		} else if (action.itemId == "trace.relative") {
			dispatchAction(ui::Action::ToggleRelativeTrails);
		} else if (action.itemId == "action.fullscreen") {
			dispatchAction(ui::Action::ToggleFullscreen);
		} else if (action.itemId == "action.resetview") {
			dispatchAction(ui::Action::ResetView);
		}
	};

	auto lastFrameTime = std::chrono::steady_clock::now();
	auto lastPredictTime = lastFrameTime;
	double fps = 0.0;
	const auto currentSimSecondsPerRealSecond = [&engine, &mpRenderFrame, &config, &mpSessionJoined,
	                                              &mpSim]() {
		if (mpRenderFrame.has_value()) {
			const net::MpClientRenderPublish& p = *mpRenderFrame;
			if (std::isfinite(p.simulatedSecondsPerRealSecond) && p.simulatedSecondsPerRealSecond > 0.0) {
				return p.simulatedSecondsPerRealSecond;
			}
			if (std::isfinite(p.config.timeScale) && p.config.timeScale > 0.0) {
				return p.config.timeScale;
			}
			return 1.0;
		}
		if (mpSessionJoined && mpSim.has_value()) {
			// Sim thread owns `engine` until first publish; avoid racing `engine` metrics here.
			if (std::isfinite(config.timeScale) && config.timeScale > 0.0) {
				return config.timeScale;
			}
			return 1.0;
		}
		const sim::SimulationConfig liveCfg = engine.config();
		const double measured = engine.simulatedSecondsPerRealSecond();
		if (std::isfinite(measured) && measured > 0.0) {
			return measured;
		}
		if (!std::isfinite(liveCfg.timeScale) || liveCfg.timeScale <= 0.0) {
			return 1.0;
		}
		return liveCfg.timeScale;
	};
	const auto panView = [&](const sf::Vector2i& pixelDelta) {
		const sf::Vector2f before = renderer.view().getCenter();
		renderer.panByPixels(pixelDelta);
		if (followViewOffset.has_value()) {
			const sf::Vector2f after = renderer.view().getCenter();
			followViewOffset->first += static_cast<double>(after.x - before.x);
			followViewOffset->second += static_cast<double>(after.y - before.y);
		}
	};

	while (window.isOpen()) {
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
				if (leftPanning && !ui.menu.active()) {
					const sf::Vector2i delta = moved->position - leftPanPixel;
					panView(delta);
					leftPanPixel = moved->position;
					const float dragDx = static_cast<float>(moved->position.x - leftDownPixel.x);
					const float dragDy = static_cast<float>(moved->position.y - leftDownPixel.y);
					if (std::sqrt(dragDx * dragDx + dragDy * dragDy) > 10.0f) {
						leftDragMaySelect = false;
					}
				}
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
					leftPanning = true;
					leftDragMaySelect = true;
					leftPanPixel = mousePressed->position;
					leftDownPixel = mousePressed->position;
				} else if (mousePressed->button == sf::Mouse::Button::Right) {
					(void)world;
				}
			}
			if (const auto* mouseReleased = event->getIf<sf::Event::MouseButtonReleased>()) {
				if (mouseReleased->button == sf::Mouse::Button::Middle) {
					middlePanning = false;
				}
				if (mouseReleased->button == sf::Mouse::Button::Left) {
					if (leftPanning && leftDragMaySelect && !ui.menu.active()) {
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
						    (mpSessionJoined && mpSim.has_value())
						        ? ui.selection.pickFromBodies(bodies, world, worldToScreenDistance,
						                                      mpOwnShipId, 26.0f)
						        : ui.selection.pick(engine, world, worldToScreenDistance,
						                            mpOwnShipId, 26.0f);
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
					if (!ui.menu.active()) {
						openMenu(true);
					}
					continue;
				}

				const std::optional<ui::Action> action =
				    ui.input.mapKeyPress(key->code, ui.menu.active());
				if (action.has_value()) {
					dispatchAction(*action);
				}
			}
		}
		const ui::HoldAdjustments holds =
		    ui.input.computeHolds(frameDt, !ui.menu.active(), !multiplayer);
		if (holds.panPixelsX != 0.0 || holds.panPixelsY != 0.0) {
			panView(sf::Vector2i(static_cast<int>(std::lround(holds.panPixelsX)),
			                     static_cast<int>(std::lround(holds.panPixelsY))));
		}
		const sf::Vector2f viewCenterBeforeUpdate = renderer.view().getCenter();
		renderer.update(frameDt);
		if (followViewOffset.has_value()) {
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
				std::optional<std::pair<double, double>> ownShipCenter;
				if (joinOwn != 0) {
					const auto ownShipIt =
					    std::find_if(joinBodies.begin(), joinBodies.end(),
					                 [&](const sim::AuthoritativeBody& b) { return b.id == joinOwn; });
					if (ownShipIt != joinBodies.end()) {
						ownShipCenter = {ownShipIt->x, ownShipIt->y};
					}
				}
				std::vector<sim::SpawnCommand> fitBodies;
				fitBodies.reserve(joinBodies.size());
				for (const sim::AuthoritativeBody& ab : joinBodies) {
					fitBodies.push_back(sim::SpawnCommand{
					    .x = ab.x,
					    .y = ab.y,
					    .vx = ab.vx,
					    .vy = ab.vy,
					    .mass = ab.mass,
					    .radius = ab.radius,
					    .name = ab.name,
					});
				}
				fitViewToBodies(renderer, fitBodies);
				mpOwnShipId = joinOwn;
				mpSessionJoined = true;
				mpSim.emplace(engine);
				mpSim->syncJoin(joinGlobalPhysicsStep, joinTimeScale, std::move(joinBodies));
				mpSim->start();
				if (mpOwnShipId != 0 && ownShipCenter.has_value()) {
					renderer.setWorldOriginForRendering(ownShipCenter->first, ownShipCenter->second);
					renderer.setCameraWorldCenterDouble(ownShipCenter->first, ownShipCenter->second);
				}
				trackedFollowId = mpOwnShipId;
				followViewOffset = {0.0, 0.0};
				mpHudServerTick = joinTick;
				setStatus("Joined multiplayer session.", 2.5);
			}
			if (mpSessionJoined) {
				if (mpSim.has_value() && !mpClient->isConnected()) {
					mpSim->stop();
					mpSim.reset();
					mpSessionJoined = false;
				} else {
				bool hadMerge = false;
				std::uint64_t mergeTick = 0;
				std::vector<std::pair<sim::BodyId, sim::BodyId>> netMerges;
				mpClient->takeMergeRemaps(mergeTick, netMerges, hadMerge);
				if (hadMerge) {
					ui.selection.applyMergeRemap(netMerges);
					ui.traces.applyMergeRemap(netMerges);
					if (mpSim.has_value()) {
						mpSim->postMergeDeletes(std::move(netMerges));
					}
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

				if (mpSim.has_value()) {
					mpSim->syncReplicas(mpShipReplica);
				}

				if (mpOwnShipId != 0) {
					double facing = 0.0;
					const render::Renderer::WorldCoordsD mouseWorld =
					    renderer.screenToWorldD(sf::Mouse::getPosition(window));
					const auto shipIt = std::find_if(
					    bodies.begin(), bodies.end(),
					    [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
					if (shipIt != bodies.end()) {
						facing = std::atan2(mouseWorld.y - shipIt->y, mouseWorld.x - shipIt->x);
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
		}

		mpRenderFrame.reset();
		if (mpSessionJoined && mpSim.has_value()) {
			net::MpClientRenderPublish pub;
			mpSim->copyLatestRenderPublish(pub);
			mpRenderFrame = std::move(pub);
			bodies = mpRenderFrame->bodies;
		}

		if (multiplayer && mpSessionJoined && mpSim.has_value()) {
			const bool simPaused =
			    mpRenderFrame.has_value() && mpRenderFrame->config.paused;
			if (!simPaused) {
				for (const net::MpClient::ShipNetSample& s : mpInboundShips) {
					mpHudServerTick = std::max(mpHudServerTick, s.serverTick);
				}
				if (mpPendingWorldHad) {
					mpHudServerTick = std::max(mpHudServerTick, mpPendingWorldTick);
				}

				// One server tick bundle (`WorldDynamicSnapshot`) carries dynamics for every body at
				// the same `globalPhysicsStep`; ship packets are display/replica only.
				const std::uint64_t lastAuth = mpSim->lastConfirmedAuthorityStep();
				std::uint64_t commitStep = lastAuth;
				std::vector<sim::BodyDynamicsPatch> authorityPatches;
				if (mpPendingWorldHad) {
					const std::uint64_t W = mpPendingWorldGlobalStep;
					commitStep = std::max(commitStep, W);
					authorityPatches.reserve(mpPendingWorldIds.size());
					for (std::size_t i = 0; i < mpPendingWorldIds.size(); ++i) {
						authorityPatches.push_back(sim::BodyDynamicsPatch{
						    .id = mpPendingWorldIds[i],
						    .x = mpPendingWorldPx[i],
						    .y = mpPendingWorldPy[i],
						    .vx = mpPendingWorldVx[i],
						    .vy = mpPendingWorldVy[i],
						});
					}
				}

				if (!authorityPatches.empty()) {
					mpSim->postAuthorityBundle(std::move(authorityPatches), commitStep);
				} else if (commitStep != lastAuth) {
					mpSim->setLastConfirmedAuthorityStep(commitStep);
				}
			}
		}

		sim::SimulationConfig cfg{};
		double traceSimClock = 0.0;
		if (mpSessionJoined && mpSim.has_value()) {
			mpSim->takePendingMergeRemaps(mergeRemapEvents);
			if (mpRenderFrame.has_value()) {
				cfg = mpRenderFrame->config;
				traceSimClock = mpRenderFrame->simulationTimeSeconds;
			} else {
				cfg = config;
			}
		} else {
			cfg = engine.config();
			traceSimClock = engine.simulationTimeSeconds();
			engine.drainMergeRemapEvents(mergeRemapEvents);
			engine.copyBodies(bodies);
		}
		ui.selection.applyMergeRemap(mergeRemapEvents);
		ui.traces.applyMergeRemap(mergeRemapEvents);
		if (mpSessionJoined && mpSim.has_value()) {
			ui.selection.validateAgainstBodies(bodies);
		} else {
			ui.selection.validateAgainstEngine(engine);
		}
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
		if (selectedBody.has_value() && mpOwnShipId != 0 && selectedBody->id == mpOwnShipId) {
			selectedBody.reset();
			ui.selection.clear();
		}
		bool followCamNow = true;
		sim::BodyId followTargetId = 0;
		double followX = 0.0;
		double followY = 0.0;
		if (ui.followCameraMode == ui::UiState::FollowCameraMode::FollowOwnShip) {
			if (mpOwnShipId != 0) {
				if (const auto it = std::find_if(
				        bodies.begin(), bodies.end(),
				        [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
				    it != bodies.end()) {
					followTargetId = it->id;
					followX = it->x;
					followY = it->y;
				} else {
					followCamNow = false;
				}
			} else {
				followCamNow = false;
			}
		} else {
			if (selectedBody.has_value()) {
				followTargetId = selectedBody->id;
				followX = selectedBody->x;
				followY = selectedBody->y;
			}
		}
		if (followCamNow) {
			renderer.setWorldOriginForRendering(followX, followY);
			if (!trackedFollowId.has_value() || *trackedFollowId != followTargetId ||
			    !followViewOffset.has_value()) {
				const render::Renderer::WorldCoordsD cc = renderer.cameraWorldCenter();
				followViewOffset = {cc.x - followX, cc.y - followY};
			}
			renderer.setCameraWorldCenterDouble(followX + followViewOffset->first,
			                                    followY + followViewOffset->second);
			trackedFollowId = followTargetId;
		} else {
			if (wasFollowCamera) {
				renderer.clearWorldRenderingOrigin();
			}
			trackedFollowId.reset();
			followViewOffset.reset();
		}
		wasFollowCamera = followCamNow;
		{
			auto s = ui.traces.settings();
			const std::size_t bodyCount = std::max<std::size_t>(1, bodies.size());
			if (bodyCount <= 32) {
				s.maxPointsPerBody = 2400;
			} else if (bodyCount <= 96) {
				s.maxPointsPerBody = 1600;
			} else if (bodyCount <= 256) {
				s.maxPointsPerBody = 1000;
			} else if (bodyCount <= 512) {
				s.maxPointsPerBody = 700;
			} else {
				s.maxPointsPerBody = 450;
			}
			// Relative-to-selected rendering is very sensitive to simplification in world-space.
			// Keep raw points while a body is selected to avoid local jitter.
			s.simplify = !selectedBody.has_value();
			ui.traces.setSettings(s);
		}
		ui.traces.ingest(bodies, traceSimClock);
		const render::Renderer::WorldCoordsD cursorWorldD =
		    renderer.screenToWorldD(sf::Mouse::getPosition(window));
		std::optional<sim::BodySnapshot> hoveredBody;
		const double worldUnitsPerPixel =
		    std::max(1e-9, static_cast<double>(renderer.worldUnitsPerPixel()));
		const double hoverMaxDistancePx = 26.0;
		double bestSurfaceDistancePx = hoverMaxDistancePx;
		double bestMass = -1.0;
		for (const sim::BodySnapshot& b : bodies) {
			if (mpOwnShipId != 0 && b.id == mpOwnShipId) {
				continue;
			}
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
		    (mpRenderFrame.has_value() && mpRenderFrame->simulatedSecondsPerUpdate > 0.0)
		        ? mpRenderFrame->simulatedSecondsPerUpdate
		        : ((mpSessionJoined && mpSim.has_value())
		               ? ((std::isfinite(cfg.timeScale) && cfg.timeScale > 0.0) ? cfg.timeScale : 1.0)
		               : ((engine.simulatedSecondsPerUpdate() > 0.0) ? engine.simulatedSecondsPerUpdate()
		                                                             : ((std::isfinite(cfg.timeScale) &&
		                                                                 cfg.timeScale > 0.0)
		                                                                    ? cfg.timeScale
		                                                                    : 1.0)));
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
		const double predictInterval = predictorSettings.predictionRecalcIntervalSeconds;
		const bool dueByInterval =
		    predictInterval <= 0.0 ||
		    std::chrono::duration<double>(now - lastPredictTime).count() >= predictInterval;
		const bool shouldRefreshPrediction = ui.showShipSelfPrediction && dueByInterval;
		if (shouldRefreshPrediction) {
			lastPredictTime = now;
			shipPredictionOffsets.clear();
			shipPredictionBodyId.reset();
			if (mpOwnShipId != 0) {
				const std::vector<sf::Vector2f> shipPredicted = ui.predictor.predictForBody(
				    bodies, mpOwnShipId, cfg.gravitationalConstant, cfg.softeningEpsilon);
				if (!shipPredicted.empty()) {
					if (const auto it = std::find_if(
					        bodies.begin(), bodies.end(),
					        [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
					    it != bodies.end()) {
						const sf::Vector2f shipAnchor(static_cast<float>(it->x),
						                              static_cast<float>(it->y));
						shipPredictionOffsets.reserve(shipPredicted.size());
						if (selectedBody.has_value() && selectedBody->id != mpOwnShipId) {
							const std::vector<sf::Vector2f> selectedPredicted =
							    ui.predictor.predictForBody(bodies, selectedBody->id,
							                                cfg.gravitationalConstant,
							                                cfg.softeningEpsilon);
							const sf::Vector2f selectedAnchor(static_cast<float>(selectedBody->x),
							                                  static_cast<float>(selectedBody->y));
							const std::size_t n =
							    std::min(shipPredicted.size(), selectedPredicted.size());
							if (n > 0) {
								for (std::size_t i = 0; i < n; ++i) {
									const sf::Vector2f rel =
									    (shipPredicted[i] - selectedPredicted[i]) + selectedAnchor;
									shipPredictionOffsets.push_back(rel - shipAnchor);
								}
							} else {
								for (const sf::Vector2f& point : shipPredicted) {
									shipPredictionOffsets.push_back(point - shipAnchor);
								}
							}
						} else {
							for (const sf::Vector2f& point : shipPredicted) {
								shipPredictionOffsets.push_back(point - shipAnchor);
							}
						}
						shipPredictionBodyId = mpOwnShipId;
					}
				}
			}
		}
		shipPrediction.clear();
		if (ui.showShipSelfPrediction && shipPredictionBodyId.has_value() &&
		    !shipPredictionOffsets.empty()) {
			if (const auto it = std::find_if(
			        bodies.begin(), bodies.end(),
			        [&](const sim::BodySnapshot& b) { return b.id == *shipPredictionBodyId; });
			    it != bodies.end()) {
				const sf::Vector2f anchor(static_cast<float>(it->x), static_cast<float>(it->y));
				shipPrediction.reserve(shipPredictionOffsets.size());
				for (const sf::Vector2f& offset : shipPredictionOffsets) {
					shipPrediction.push_back(anchor + offset);
				}
			}
		}

		std::vector<ui::MenuItem> menuItems{
		    {"trace.enabled", "Trails", ui.traces.settings().enabled ? "On" : "Off", false},
		    {"trace.relative", "Trails Frame", ui.traces.settings().relative ? "Relative" : "World",
		     false},
		    {"predict.ship", "Ship Prediction", ui.showShipSelfPrediction ? "On" : "Off", false},
		    {"follow.mode", "Follow Mode",
		     ui.followCameraMode == ui::UiState::FollowCameraMode::FollowOwnShip ? "Own ship"
		                                                                         : "Selected/0,0",
		     false},
		    {"action.resetview", "Reset View", "R", false},
		    {"action.fullscreen", "Fullscreen", "F11", false},
		};
		ui.menu.setItems(menuItems);

		std::optional<float> playerFacing = std::nullopt;
		if (mpOwnShipId != 0) {
			if (const auto it = mpShipReplica.find(mpOwnShipId); it != mpShipReplica.end()) {
				playerFacing = it->second.facing;
			}
		}
		renderer.draw(bodies,
		              mpOwnShipId == 0 ? std::nullopt : std::optional<sim::BodyId>(mpOwnShipId),
		              playerFacing);
		{
			double rox = 0.0;
			double roy = 0.0;
			bool useRo = false;
			renderer.worldRenderingOrigin(rox, roy, useRo);
			std::optional<sim::BodyId> trailsRefId = std::nullopt;
			std::optional<std::pair<double, double>> trailsRefAnchor = std::nullopt;
			if (ui.traces.settings().relative) {
				if (selectedBody.has_value()) {
					trailsRefId = selectedBody->id;
					trailsRefAnchor = std::pair<double, double>{selectedBody->x, selectedBody->y};
				} else if (mpOwnShipId != 0) {
					if (const auto it = std::find_if(
					        bodies.begin(), bodies.end(),
					        [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
					    it != bodies.end()) {
						trailsRefId = it->id;
						trailsRefAnchor = std::pair<double, double>{it->x, it->y};
					}
				}
				if (!trailsRefAnchor.has_value()) {
					// Keep default-relative mode stable even before own ship resolves.
					trailsRefAnchor = std::pair<double, double>{0.0, 0.0};
				}
			}
			ui.traces.draw(window, selectedBody, trailsRefId, trailsRefAnchor, rox, roy, useRo);
		}

		const auto worldToRenderLocal = [&](double x, double y) {
			return renderer.worldToRenderLocal(x, y);
		};
		const auto worldToPixel = [&](double x, double y) { return renderer.worldToPixelD(x, y); };
		std::optional<sim::BodySnapshot> ownShipBody;
		if (mpOwnShipId != 0) {
			if (const auto it =
			        std::find_if(bodies.begin(), bodies.end(),
			                     [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
			    it != bodies.end()) {
				ownShipBody = *it;
			}
		}
		overlay.drawWorldSelection(window, ownShipBody, selectedBody, std::nullopt, shipPrediction,
		                           std::nullopt, displayTimeRate, false, false, worldToRenderLocal,
		                           worldToPixel);
		overlay.drawWorldSelection(window, selectedBody, std::nullopt, ownShipBody, {},
		                           std::nullopt, displayTimeRate, true, true, worldToRenderLocal,
		                           worldToPixel);
		overlay.drawWorldSelection(window, hoveredBody, selectedBody, selectedBody, {},
		                           std::nullopt, displayTimeRate, false, true, worldToRenderLocal,
		                           worldToPixel);

		if (std::chrono::steady_clock::now() > statusUntil) {
			ui.statusMessage.clear();
		}
		std::vector<std::string> hudLines;
		if (multiplayer) {
			if (!mpSessionJoined) {
				hudLines.push_back("Multiplayer: connecting to " + std::string(mpHost) + ":" +
				                   std::to_string(static_cast<unsigned>(mpPort)) + "...");
			} else {
				hudLines.push_back("Multiplayer: tick " + std::to_string(mpHudServerTick) +
				                   " | server: potato_gsim_server");
			}
		}
		hudLines.push_back("Bodies: " +
		                   std::to_string((mpSessionJoined && mpSim.has_value()) ? bodies.size()
		                                                                       : engine.bodyCount()));
		hudLines.push_back("Time speed: " + timeScaleDisplay);
		hudLines.push_back("Scale: " + scaleDisplay + " | Predict: " + predictWindowDisplay +
		                   " | FPS: " + std::to_string(round3(fps)));
		hudLines.push_back(
		    "Trails: " + std::string(ui.traces.settings().enabled ? "On" : "Off") + " (" +
		    std::string(ui.traces.settings().relative ? "Relative" : "World") + ")" +
		    " | Ship prediction: " + std::string(ui.showShipSelfPrediction ? "On" : "Off"));
		if (!ui.statusMessage.empty()) {
			hudLines.push_back("Status: " + ui.statusMessage);
		}
		overlay.drawHudPanel(window, hudLines, ui.input.legendLines(ui.menu.active()), false);

		if (ui.menu.active()) {
			const sf::Font* menuFont = overlay.fontPtr();
			if (menuFont != nullptr) {
				ui.menu.draw(window, *menuFont);
			}
		}

		window.display();
	}

	if (multiplayer) {
		if (mpSim.has_value()) {
			mpSim->stop();
			mpSim.reset();
		}
		mpClient.reset();
		enet_deinitialize();
	}
	engine.stop();
	return 0;
}
