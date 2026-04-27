#include "io/ClientSettings.hpp"
#include "net/MpClient.hpp"
#include "net/MpClientSim.hpp"
#include "net/MpConstants.hpp"
#include "net/Protocol.hpp"
#include "render/Renderer.hpp"
#include "scenario/ScenarioManager.hpp"
#include "sim/SimulationEngine.hpp"
#include "ui/InspectorOverlay.hpp"
#include "ui/KeyboardChordState.hpp"
#include "ui/QuantityFormat.hpp"
#include "ui/UiState.hpp"

#include <CLI11.hpp>

#include <SFML/Graphics.hpp>
#include <SFML/Window/Keyboard.hpp>

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

enum class MenuPhase { Hidden, Root, SpScenario, MpConnect, MpConnecting };

sim::SpawnCommand makeSoloPlayerShipSpawn(const std::string& shipName,
                                          const scenario::PresetKind presetKind,
                                          const scenario::RandomPresetConfig& randomCfg,
                                          const std::size_t idx) {
	const double a = static_cast<double>(idx) * 2.399963229728653;
	double cx = 0.0;
	double cy = 0.0;
	double r = 5e10 + 5e9 * static_cast<double>(idx % 6);
	switch (presetKind) {
		case scenario::PresetKind::SolarLike:
			r = 1.49598e11 + 1.5e9 * static_cast<double>(idx % 9);
			break;
		case scenario::PresetKind::BinaryDance:
			r = 4.2e11 + 2.2e9 * static_cast<double>(idx % 9);
			break;
		case scenario::PresetKind::SpiralCluster:
			r = 9.0e10 + 1.2e9 * static_cast<double>(idx % 10);
			break;
		case scenario::PresetKind::Random:
			cx = randomCfg.centerX;
			cy = randomCfg.centerY;
			r = std::max(1e8, randomCfg.spreadRadius * 0.18) + 9e7 * static_cast<double>(idx % 16);
			break;
	}
	return sim::SpawnCommand{
	    .x = cx + r * std::cos(a),
	    .y = cy + r * std::sin(a),
	    .vx = 0.0,
	    .vy = 0.0,
	    .mass = 2e4,
	    .radius = 15.0,
	    .name = shipName,
	};
}

bool parseHostPort(const std::string& line, std::string& hostOut, std::uint16_t& portOut) {
	std::string s = line;
	while (!s.empty() && (s.back() == ' ' || s.back() == '\t')) {
		s.pop_back();
	}
	while (!s.empty() && (s.front() == ' ' || s.front() == '\t')) {
		s.erase(s.begin());
	}
	if (s.empty()) {
		return false;
	}
	const std::size_t colon = s.rfind(':');
	if (colon == std::string::npos || colon == 0) {
		hostOut = s;
		portOut = 27777;
		return !hostOut.empty();
	}
	hostOut = s.substr(0, colon);
	std::string ps = s.substr(colon + 1);
	const unsigned long p = std::strtoul(ps.c_str(), nullptr, 10);
	if (p == 0 || p > 65535UL) {
		return false;
	}
	portOut = static_cast<std::uint16_t>(p);
	while (!hostOut.empty() &&
	       (hostOut.back() == ' ' || hostOut.back() == '\t' || hostOut.back() == '\r')) {
		hostOut.pop_back();
	}
	return !hostOut.empty();
}

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

[[nodiscard]] bool bodyLooksLikeDefaultPlayerShip(const sim::BodySnapshot& b) {
	constexpr double kMass = 2e4;
	constexpr double kRad = 15.0;
	return std::abs(b.mass - kMass) < 8e3 && std::abs(b.radius - kRad) < 8.0;
}

using MpShipReplica = net::MpShipReplicaInput;

}  // namespace

int main(int argc, char** argv) {
	io::ClientSettings clientSettings;
	const bool settingsLoadedFromFile = io::loadClientSettings(clientSettings);
	if (!settingsLoadedFromFile) {
#if defined(POTATO_GSIM_DEFAULT_MP_HOST)
		clientSettings.mpHost = POTATO_GSIM_DEFAULT_MP_HOST;
#endif
#if defined(POTATO_GSIM_DEFAULT_MP_PORT)
		clientSettings.mpPort = static_cast<std::uint16_t>(POTATO_GSIM_DEFAULT_MP_PORT);
#endif
	}

	bool multiplayer = false;
	std::string mpHost = clientSettings.mpHost;
	std::uint16_t mpPort = clientSettings.mpPort;
	bool startFullscreen = clientSettings.fullscreen;
	std::uint32_t windowWidth = clientSettings.windowWidth;
	std::uint32_t windowHeight = clientSettings.windowHeight;
	std::string localPreset = clientSettings.spPreset;
	scenario::RandomPresetConfig localRandomCfg = clientSettings.randomCfg;
	std::vector<std::string> connectArgs;
	std::string playerName = clientSettings.playerName;

	MenuPhase menuPhase = MenuPhase::Root;
	bool cliQuickMultiplayer = false;
	bool spSessionActive = false;
	int menuRootSel = 0;
	int spScenarioSel = 0;
	int mpConnectSel = 0;
	std::string mpAddressEdit;
	bool editingMenuText = false;
	double spShellCooldownWall = 0.0;
	// Solo ship delta-v (defaults match `main_server.cpp` ship fuel).
	constexpr double kSpMaxDeltaV = 30000.0;
	constexpr double kSpDeltaVRegenDelaySeconds = 5.0;
	constexpr double kSpDeltaVRegenPerRealSecond = 500.0;
	double spDeltaVCurrent = 0.0;
	double spWallSecondsSinceThrust = 0.0;
	bool spPendingThrustForward = false;
	double spPendingThrustScale = 0.0;
	sim::BodyId spFuelShipId = 0;

	CLI::App app{"potato_gsim client"};
	app.add_flag("--multiplayer", multiplayer, "Enable multiplayer mode (skip main menu)");
	app.add_option("--host", mpHost, "Server host")->capture_default_str();
	app.add_option("--port", mpPort, "Server port")->capture_default_str();
	app.add_option("--connect", connectArgs, "Legacy connect form: --connect <host> <port>")
	    ->expected(2);
	app.add_option("--window-width", windowWidth, "Window width")->capture_default_str();
	app.add_option("--window-height", windowHeight, "Window height")->capture_default_str();
	app.add_flag("--fullscreen", startFullscreen, "Start in fullscreen");
	app.add_option("--player-name", playerName, "Player / ship name")->capture_default_str();
	app.add_option("--preset", localPreset,
	               "Single-player startup preset: empty|solar|binary|spiral|random")
	    ->capture_default_str();
	app.add_option("--random-count", localRandomCfg.count, "Random preset body count");
	app.add_option("--random-center-x", localRandomCfg.centerX, "Random preset center X");
	app.add_option("--random-center-y", localRandomCfg.centerY, "Random preset center Y");
	app.add_option("--random-spread", localRandomCfg.spreadRadius, "Random preset spread radius");
	CLI11_PARSE(app, argc, argv);
	if (connectArgs.size() == 2) {
		multiplayer = true;
		cliQuickMultiplayer = true;
		mpHost = connectArgs[0];
		mpPort = static_cast<std::uint16_t>(std::strtoul(connectArgs[1].c_str(), nullptr, 10));
	}
	if (multiplayer) {
		cliQuickMultiplayer = true;
	}
	if (cliQuickMultiplayer) {
		menuPhase = MenuPhase::Hidden;
	}
	windowWidth = std::max<std::uint32_t>(320, windowWidth);
	windowHeight = std::max<std::uint32_t>(240, windowHeight);
	localRandomCfg.count = std::max<std::size_t>(1, localRandomCfg.count);
	localRandomCfg.spreadRadius = std::max(1.0, localRandomCfg.spreadRadius);
	if (playerName.empty()) {
		playerName = "Player";
	}

	mpAddressEdit = mpHost + ":" + std::to_string(static_cast<unsigned>(mpPort));

	sf::RenderWindow window(sf::VideoMode({windowWidth, windowHeight}), "potato_gsim",
	                        sf::Style::Default, sf::State::Windowed);
	window.setVerticalSyncEnabled(true);
	window.setFramerateLimit(0);
	if (startFullscreen) {
		setWindowFullscreen(window, true);
	}

	sim::SimulationConfig config;
	config.timeScale = 3600.0;
	config.fixedDtSeconds = net::kDefaultRealSecondsPerPhysicsStep;
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
	bool mpHaveRespawnCountdownState = false;
	std::uint64_t mpRespawnAtServerTick = 0;
	double mpRespawnSecondsPerServerTick = 0.0;
	sim::BodyId mpOwnShipId = 0;
	bool mpSessionJoined = false;
	bool mpJoinRequestSent = false;
	std::uint32_t mpInputSeq = 0;
	std::optional<net::MpClientSim> mpSim;
	std::vector<net::MpClient::ShipNetSample> mpInboundShips;
	std::unordered_map<sim::BodyId, MpShipReplica> mpShipReplica;
	sim::BodyId mpPrevOwnShipId = 0;
	/// Tracks `BodyId` for `playerName` from world snapshots (respawn changes id).
	sim::BodyId mpLastResolvedOwnShipBodyId = 0;
	bool mpShipMouseAim = clientSettings.shipMouseAim;
	double mpShipHeadingRadians = 0.0;
	bool mpShipHeadingInited = false;
	std::uint8_t mpShipThrustPercent = 100;
	float mpShipDeltaVCurrentMps = 0.0f;
	float mpShipDeltaVMaxMps = 0.0f;
	/// From last own-ship `ShipState`: physics step index in that message and first step shell may
	/// fire again.
	std::uint64_t mpOwnShipStateGlobalPhysicsStep = 0;
	std::uint64_t mpShellReadyGlobalPhysicsStep = 0;
	/// Server/client sim `timeScale` (sim s / real s) for encoding shell speed in inputs.
	double mpNetPhysicsTimeScale = 1.0;
	std::uint64_t mpNetPrevInboundDropsTotal = 0;
	std::uint64_t mpNetPrevSimCapHitsTotal = 0;
	std::optional<std::chrono::steady_clock::time_point> mpLastClientStressLog;

	if (enet_initialize() != 0) {
		return 1;
	}
	if (cliQuickMultiplayer) {
		mpClient.emplace();
		if (!mpClient->connect(mpHost, mpPort)) {
			std::fprintf(stderr, "potato_gsim: could not connect to %s:%u\n", mpHost.c_str(),
			             static_cast<unsigned>(mpPort));
			enet_deinitialize();
			return 1;
		}
	}

	render::Renderer renderer(window);
	renderer.onResize(window.getSize());
	ui::InspectorOverlay overlay;
	ui::UiState ui;
	ui::KeyboardChordState keysHeld{};
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
	    .maxAttractors = 10,
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

	bool middlePanning = false;
	sf::Vector2i lastPanPixel{0, 0};
	bool leftPanning = false;
	bool leftDragMaySelect = false;
	sf::Vector2i leftPanPixel{0, 0};
	sf::Vector2i leftDownPixel{0, 0};
	bool fullscreen = startFullscreen;

	auto persistClientSettingsToDisk = [&]() {
		clientSettings.playerName = playerName;
		clientSettings.mpHost = mpHost;
		clientSettings.mpPort = mpPort;
		clientSettings.fullscreen = fullscreen;
		clientSettings.shipMouseAim = mpShipMouseAim;
		clientSettings.windowWidth = window.getSize().x;
		clientSettings.windowHeight = window.getSize().y;
		clientSettings.spPreset = localPreset;
		clientSettings.randomCfg = localRandomCfg;
		(void)io::saveClientSettings(clientSettings, nullptr);
	};

	std::vector<sim::BodySnapshot> bodies;
	std::vector<std::pair<sim::BodyId, sim::BodyId>> mergeRemapEvents;
	std::vector<sf::Vector2f> shipPrediction;
	std::vector<sf::Vector2f> shipPredictionOffsets;
	std::optional<sim::BodyId> shipPredictionBodyId;
	bool shipPredictionStoppedOnEncounter = false;
	std::vector<sf::Vector2f> shellPrediction;
	/// Reference body id for `shellPrediction` offsets (selected if any, else firer).
	std::optional<sim::BodyId> shellPredictionAnchorBodyId;
	std::optional<sim::BodyId> trackedFollowId;
	std::optional<std::pair<double, double>> followViewOffset;
	bool wasFollowCamera = false;
	std::optional<net::MpClientRenderPublish> mpRenderFrame;

	const auto resolveMpOwnShipAndCamera = [&](const std::vector<sim::BodySnapshot>& snap) {
		if (!mpSessionJoined) {
			return;
		}
		const auto it = std::find_if(snap.begin(), snap.end(), [&](const sim::BodySnapshot& b) {
			return b.name == playerName;
		});
		if (it == snap.end()) {
			mpLastResolvedOwnShipBodyId = 0;
			return;
		}
		if (mpOwnShipId != it->id) {
			mpOwnShipId = it->id;
		}
		if (mpLastResolvedOwnShipBodyId != it->id) {
			renderer.setWorldOriginForRendering(it->x, it->y);
			renderer.setCameraWorldCenterDouble(it->x, it->y);
			trackedFollowId = it->id;
			followViewOffset = {0.0, 0.0};
			mpShipHeadingInited = false;
			mpLastResolvedOwnShipBodyId = it->id;
		}
	};

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
				persistClientSettingsToDisk();
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
			case ui::Action::ToggleShellPrediction:
				ui.showShellPrediction = !ui.showShellPrediction;
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
		} else if (action.itemId == "predict.shell") {
			ui.showShellPrediction = !ui.showShellPrediction;
		} else if (action.itemId == "follow.mode") {
			dispatchAction(ui::Action::ToggleFollowMode);
		} else if (action.itemId == "trace.relative") {
			dispatchAction(ui::Action::ToggleRelativeTrails);
		} else if (action.itemId == "action.fullscreen") {
			dispatchAction(ui::Action::ToggleFullscreen);
		} else if (action.itemId == "action.resetview") {
			dispatchAction(ui::Action::ResetView);
		} else if (action.itemId == "mp.disconnect") {
			openMenu(false);
			if (mpSim.has_value()) {
				mpSim->stop();
				mpSim.reset();
			}
			mpHaveRespawnCountdownState = false;
			mpRespawnAtServerTick = 0;
			mpRespawnSecondsPerServerTick = 0.0;
			mpClient.reset();
			mpJoinRequestSent = false;
			mpSessionJoined = false;
			mpShipReplica.clear();
			mpPrevOwnShipId = 0;
			mpLastResolvedOwnShipBodyId = 0;
			mpOwnShipId = 0;
			multiplayer = false;
			menuPhase = MenuPhase::Root;
			persistClientSettingsToDisk();
		} else if (action.itemId == "sp.exit") {
			openMenu(false);
			engine.stop();
			spSessionActive = false;
			mpOwnShipId = 0;
			spFuelShipId = 0;
			spDeltaVCurrent = 0.0;
			spWallSecondsSinceThrust = 0.0;
			spPendingThrustForward = false;
			(void)engine.takeAccumulatedSimulatedSecondsForShipFuel();
			trackedFollowId.reset();
			followViewOffset.reset();
			menuPhase = MenuPhase::Root;
			persistClientSettingsToDisk();
		}
	};

	auto lastFrameTime = std::chrono::steady_clock::now();
	auto lastPredictTime = lastFrameTime;
	double fps = 0.0;
	const auto currentSimSecondsPerRealSecond = [&engine, &mpRenderFrame, &config, &mpSessionJoined,
	                                             &mpSim]() {
		if (mpRenderFrame.has_value()) {
			const net::MpClientRenderPublish& p = *mpRenderFrame;
			if (std::isfinite(p.simulatedSecondsPerRealSecond) &&
			    p.simulatedSecondsPerRealSecond > 0.0) {
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

		if (menuPhase != MenuPhase::Hidden) {
			bool menuKb = window.hasFocus();
			while (const auto event = window.pollEvent()) {
				if (event->is<sf::Event::Closed>()) {
					persistClientSettingsToDisk();
					window.close();
					break;
				}
				if (const auto* txt = event->getIf<sf::Event::TextEntered>()) {
					const char32_t ch = txt->unicode;
					if (ch < 32 || ch == 127) {
						continue;
					}
					if (menuPhase == MenuPhase::Root && menuRootSel == 0 && editingMenuText) {
						if (playerName.size() <
						    static_cast<std::size_t>(net::kJoinRequestNameMaxBytes)) {
							if (ch < 128) {
								playerName.push_back(static_cast<char>(ch));
							}
						}
					} else if (menuPhase == MenuPhase::MpConnect && mpConnectSel == 0) {
						if (mpAddressEdit.size() < 200 && ch < 128) {
							mpAddressEdit.push_back(static_cast<char>(ch));
						}
					}
					continue;
				}
				if (const auto* key = event->getIf<sf::Event::KeyPressed>()) {
					if (!menuKb) {
						continue;
					}
					if (key->code == sf::Keyboard::Key::Escape) {
						if (menuPhase == MenuPhase::SpScenario ||
						    menuPhase == MenuPhase::MpConnect ||
						    menuPhase == MenuPhase::MpConnecting) {
							if (menuPhase == MenuPhase::MpConnecting) {
								if (mpSim.has_value()) {
									mpSim->stop();
									mpSim.reset();
								}
								mpClient.reset();
								mpJoinRequestSent = false;
								mpSessionJoined = false;
								multiplayer = false;
							}
							menuPhase = MenuPhase::Root;
							editingMenuText = false;
						} else {
							persistClientSettingsToDisk();
							window.close();
						}
						continue;
					}
					if (key->code == sf::Keyboard::Key::Backspace) {
						if (editingMenuText) {
							if (menuPhase == MenuPhase::Root && menuRootSel == 0 &&
							    !playerName.empty()) {
								playerName.pop_back();
							} else if (menuPhase == MenuPhase::MpConnect && mpConnectSel == 0 &&
							           !mpAddressEdit.empty()) {
								mpAddressEdit.pop_back();
							}
						}
						continue;
					}
					if (key->code == sf::Keyboard::Key::Enter) {
						if (menuPhase == MenuPhase::Root) {
							if (menuRootSel == 0) {
								editingMenuText = !editingMenuText;
							} else if (menuRootSel == 1) {
								menuPhase = MenuPhase::SpScenario;
								spScenarioSel = 0;
							} else if (menuRootSel == 2) {
								menuPhase = MenuPhase::MpConnect;
								mpConnectSel = 0;
								mpAddressEdit =
								    mpHost + ":" + std::to_string(static_cast<unsigned>(mpPort));
							} else if (menuRootSel == 3) {
								persistClientSettingsToDisk();
								window.close();
							}
						} else if (menuPhase == MenuPhase::SpScenario) {
							if (spScenarioSel >= 0 && spScenarioSel <= 4) {
								static const char* kPresets[] = {"empty", "solar", "binary",
								                                 "spiral", "random"};
								localPreset = kPresets[static_cast<std::size_t>(spScenarioSel)];
								config.timeScale = 3600.0;
								if (localPreset == "solar") {
									applyScenario(scenario::ScenarioManager::makePreset(
									    scenario::PresetKind::SolarLike));
								} else if (localPreset == "binary") {
									applyScenario(scenario::ScenarioManager::makePreset(
									    scenario::PresetKind::BinaryDance));
								} else if (localPreset == "spiral") {
									applyScenario(scenario::ScenarioManager::makePreset(
									    scenario::PresetKind::SpiralCluster));
								} else if (localPreset == "random") {
									applyScenario(
									    scenario::ScenarioManager::makeRandom(localRandomCfg));
								} else {
									applyScenario({});
								}
								scenario::PresetKind shipPlace = scenario::PresetKind::SolarLike;
								if (localPreset == "binary") {
									shipPlace = scenario::PresetKind::BinaryDance;
								} else if (localPreset == "spiral") {
									shipPlace = scenario::PresetKind::SpiralCluster;
								} else if (localPreset == "random") {
									shipPlace = scenario::PresetKind::Random;
								}
								const sim::SpawnCommand ship = makeSoloPlayerShipSpawn(
								    playerName, shipPlace, localRandomCfg, 0);
								engine.queueSpawn(ship);
								engine.start();
								multiplayer = false;
								mpOwnShipId = 0;
								spFuelShipId = 0;
								spShellCooldownWall = 0.0;
								spSessionActive = true;
								menuPhase = MenuPhase::Hidden;
								editingMenuText = false;
								persistClientSettingsToDisk();
							} else if (spScenarioSel == 5) {
								menuPhase = MenuPhase::Root;
							}
						} else if (menuPhase == MenuPhase::MpConnect) {
							if (mpConnectSel == 0) {
								editingMenuText = !editingMenuText;
							} else if (mpConnectSel == 1) {
								std::string h;
								std::uint16_t p = 27777;
								if (!parseHostPort(mpAddressEdit, h, p)) {
									setStatus("Invalid address (use host:port)", 2.5);
								} else {
									mpHost = h;
									mpPort = p;
									mpClient.emplace();
									if (!mpClient->connect(mpHost, mpPort)) {
										setStatus("Connection failed", 2.5);
										mpClient.reset();
									} else {
										menuPhase = MenuPhase::MpConnecting;
										mpJoinRequestSent = false;
										multiplayer = true;
										config.timeScale = 1.0;
									}
								}
							} else if (mpConnectSel == 2) {
								menuPhase = MenuPhase::Root;
							}
						}
						continue;
					}
					if (key->code == sf::Keyboard::Key::Up) {
						editingMenuText = false;
						if (menuPhase == MenuPhase::Root) {
							menuRootSel = std::max(0, menuRootSel - 1);
						} else if (menuPhase == MenuPhase::SpScenario) {
							spScenarioSel = std::max(0, spScenarioSel - 1);
						} else if (menuPhase == MenuPhase::MpConnect) {
							mpConnectSel = std::max(0, mpConnectSel - 1);
						}
						continue;
					}
					if (key->code == sf::Keyboard::Key::Down) {
						editingMenuText = false;
						if (menuPhase == MenuPhase::Root) {
							menuRootSel = std::min(3, menuRootSel + 1);
						} else if (menuPhase == MenuPhase::SpScenario) {
							spScenarioSel = std::min(5, spScenarioSel + 1);
						} else if (menuPhase == MenuPhase::MpConnect) {
							mpConnectSel = std::min(2, mpConnectSel + 1);
						}
						continue;
					}
				}
			}

			if (menuPhase == MenuPhase::MpConnecting && mpClient.has_value()) {
				mpClient->service(0);
				if (!mpClient->isConnected()) {
					setStatus("Disconnected", 2.5);
					mpClient.reset();
					menuPhase = MenuPhase::MpConnect;
					multiplayer = false;
					mpJoinRequestSent = false;
				} else {
					if (!mpJoinRequestSent && mpClient->isPeerConnected()) {
						mpClient->sendJoinRequest(playerName);
						mpJoinRequestSent = true;
					}
					net::JoinRejectReason rj = net::JoinRejectReason::NameInvalid;
					std::string rjDetail;
					if (mpClient->takeJoinReject(rj, rjDetail)) {
						std::string msg = rjDetail.empty() ? "Join rejected" : rjDetail;
						setStatus(msg, 4.0);
						if (mpSim.has_value()) {
							mpSim->stop();
							mpSim.reset();
						}
						mpClient.reset();
						mpJoinRequestSent = false;
						mpSessionJoined = false;
						multiplayer = false;
						menuPhase = MenuPhase::MpConnect;
					}
					std::uint64_t joinTick = 0;
					std::uint64_t joinGlobalPhysicsStep = 0;
					std::vector<sim::AuthoritativeBody> joinBodies;
					sim::BodyId joinOwn = 0;
					double joinTimeScale = 1.0;
					double joinPhysicsRealStep = net::kDefaultRealSecondsPerPhysicsStep;
					double joinShipThrustAccel = net::kDefaultShipThrustAccel;
					mpClient->takeJoinAccept(joinTick, joinGlobalPhysicsStep, joinBodies, joinOwn,
					                         joinTimeScale, joinPhysicsRealStep,
					                         joinShipThrustAccel);
					if (!joinBodies.empty()) {
						std::optional<std::pair<double, double>> ownShipCenter;
						if (joinOwn != 0) {
							const auto ownShipIt = std::find_if(
							    joinBodies.begin(), joinBodies.end(),
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
						mpSim->syncJoin(joinGlobalPhysicsStep, joinTimeScale, joinPhysicsRealStep,
						                joinShipThrustAccel, std::move(joinBodies));
						mpSim->start();
						if (mpOwnShipId != 0 && ownShipCenter.has_value()) {
							renderer.setWorldOriginForRendering(ownShipCenter->first,
							                                    ownShipCenter->second);
							renderer.setCameraWorldCenterDouble(ownShipCenter->first,
							                                    ownShipCenter->second);
						}
						trackedFollowId = mpOwnShipId;
						followViewOffset = {0.0, 0.0};
						mpHudServerTick = joinTick;
						mpNetPhysicsTimeScale =
						    (std::isfinite(joinTimeScale) && joinTimeScale > 0.0) ? joinTimeScale
						                                                          : 1.0;
						mpNetPrevInboundDropsTotal = mpClient->inboundPacketsDroppedTotal();
						mpNetPrevSimCapHitsTotal = mpSim->simPhysicsWakeCapHitsTotal();
						mpLastClientStressLog.reset();
						setStatus("Joined multiplayer session.", 2.5);
						menuPhase = MenuPhase::Hidden;
						editingMenuText = false;
						persistClientSettingsToDisk();
					}
				}
			}

			// In-game `Renderer::draw` leaves the window on a zoomed world `sf::View`. The
			// full-screen main menu text uses pixel positions; draw it in a default-sized view
			// (same idea as `ui::MenuOverlay::draw`) or it lands in world space and is invisible.
			const sf::View menuSavedView = window.getView();
			{
				const sf::Vector2u winSize = window.getSize();
				const sf::Vector2f fsz(static_cast<float>(std::max(1u, winSize.x)),
				                       static_cast<float>(std::max(1u, winSize.y)));
				sf::View uiView(sf::FloatRect(sf::Vector2f(0.0f, 0.0f), fsz));
				uiView.setViewport(
				    sf::FloatRect(sf::Vector2f(0.0f, 0.0f), sf::Vector2f(1.0f, 1.0f)));
				window.setView(uiView);
			}

			const sf::Font* f = overlay.fontPtr();
			window.clear(sf::Color(24, 28, 32));
			if (f != nullptr) {
				float y = 40.f;
				const float lh = 22.f;
				auto line = [&](const std::string& s, const sf::Color& col, bool sel) {
					sf::Text t(*f, s, sel ? 18u : 16u);
					t.setFillColor(col);
					t.setPosition({40.f, y});
					window.draw(t);
					y += lh;
				};
				line("potato_gsim - main menu", sf::Color(220, 220, 230), false);
				y += 6;
				if (menuPhase == MenuPhase::Root) {
					line(std::string("Name: ") + playerName +
					         (editingMenuText && menuRootSel == 0 ? "_" : ""),
					     menuRootSel == 0 ? sf::Color::Yellow : sf::Color::White, menuRootSel == 0);
					line("Singleplayer", menuRootSel == 1 ? sf::Color::Yellow : sf::Color::White,
					     menuRootSel == 1);
					line("Multiplayer", menuRootSel == 2 ? sf::Color::Yellow : sf::Color::White,
					     menuRootSel == 2);
					line("Quit", menuRootSel == 3 ? sf::Color::Yellow : sf::Color::White,
					     menuRootSel == 3);
					y += 8;
					line("Up/Down select  Enter activate  Esc quit", sf::Color(160, 160, 170),
					     false);
				} else if (menuPhase == MenuPhase::SpScenario) {
					line("Choose scenario (Enter = start, Esc = back)", sf::Color(200, 200, 210),
					     false);
					y += 4;
					const char* labels[] = {"Empty", "Solar", "Binary", "Spiral", "Random", "Back"};
					for (int i = 0; i <= 5; ++i) {
						line(labels[i], spScenarioSel == i ? sf::Color::Yellow : sf::Color::White,
						     spScenarioSel == i);
					}
				} else if (menuPhase == MenuPhase::MpConnect) {
					line("Multiplayer - server address", sf::Color(200, 200, 210), false);
					y += 4;
					line(std::string("Address: ") + mpAddressEdit +
					         (editingMenuText && mpConnectSel == 0 ? "_" : ""),
					     mpConnectSel == 0 ? sf::Color::Yellow : sf::Color::White,
					     mpConnectSel == 0);
					line("Connect", mpConnectSel == 1 ? sf::Color::Yellow : sf::Color::White,
					     mpConnectSel == 1);
					line("Back", mpConnectSel == 2 ? sf::Color::Yellow : sf::Color::White,
					     mpConnectSel == 2);
				} else if (menuPhase == MenuPhase::MpConnecting) {
					line("Connecting to " + mpHost + ":" +
					         std::to_string(static_cast<unsigned>(mpPort)) + "...",
					     sf::Color::White, false);
					line("(Join in progress)", sf::Color(180, 180, 190), false);
				}
				if (!ui.statusMessage.empty()) {
					y += 10;
					sf::Text st(*f, "Status: " + ui.statusMessage, 15u);
					st.setFillColor(sf::Color(255, 140, 100));
					st.setPosition(
					    {40.f, std::min(y, static_cast<float>(window.getSize().y) - 60.f)});
					window.draw(st);
				}
			}
			window.setView(menuSavedView);
			window.display();
			continue;
		}

		bool windowKeyboardActive = window.hasFocus();
		while (const auto event = window.pollEvent()) {
			if (event->is<sf::Event::Closed>()) {
				persistClientSettingsToDisk();
				window.close();
				continue;
			}
			if (event->is<sf::Event::FocusGained>()) {
				windowKeyboardActive = true;
				continue;
			}
			if (event->is<sf::Event::FocusLost>()) {
				windowKeyboardActive = false;
				keysHeld.clear();
				continue;
			}
			if (const auto* keyReleased = event->getIf<sf::Event::KeyReleased>()) {
				keysHeld.setDown(keyReleased->code, false);
			}
			if (const auto* resized = event->getIf<sf::Event::Resized>()) {
				(void)resized;
				renderer.onResize(window.getSize());
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
				if (!windowKeyboardActive) {
					continue;
				}
				keysHeld.setDown(key->code, true);
				if (!ui.menu.active() && mpOwnShipId != 0 &&
				    ((multiplayer && mpSessionJoined) || (!multiplayer && spSessionActive)) &&
				    key->code == sf::Keyboard::Key::M) {
					mpShipMouseAim = !mpShipMouseAim;
					if (!mpShipMouseAim) {
						const render::Renderer::WorldCoordsD mouseWorld =
						    renderer.screenToWorldD(sf::Mouse::getPosition(window));
						const auto shipIt = std::find_if(
						    bodies.begin(), bodies.end(),
						    [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
						if (shipIt != bodies.end()) {
							mpShipHeadingRadians =
							    std::atan2(mouseWorld.y - shipIt->y, mouseWorld.x - shipIt->x);
						} else if (const auto it = mpShipReplica.find(mpOwnShipId);
						           it != mpShipReplica.end()) {
							mpShipHeadingRadians = static_cast<double>(it->second.facing);
						}
						mpShipHeadingInited = true;
					}
					clientSettings.shipMouseAim = mpShipMouseAim;
					setStatus(mpShipMouseAim ? "Ship aim: mouse (M toggles)"
					                         : "Ship aim: A/D (M toggles)",
					          1.6);
					persistClientSettingsToDisk();
					continue;
				}
				if (!ui.menu.active() && mpOwnShipId != 0 &&
				    ((multiplayer && mpSessionJoined) || (!multiplayer && spSessionActive)) &&
				    key->code == sf::Keyboard::Key::X) {
					mpShipThrustPercent = 0;
					setStatus("Thrust power: 0%", 0.9);
					continue;
				}
				if (!ui.menu.active() && mpOwnShipId != 0 &&
				    ((multiplayer && mpSessionJoined) || (!multiplayer && spSessionActive)) &&
				    key->code == sf::Keyboard::Key::Z) {
					mpShipThrustPercent = 100;
					setStatus("Thrust power: 100%", 0.9);
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
		const bool shipControlSession =
		    (!multiplayer && spSessionActive) || (multiplayer && mpSessionJoined);
		const ui::HoldAdjustments holds = ui.input.computeHolds(
		    frameDt, !ui.menu.active() && windowKeyboardActive, !shipControlSession, keysHeld);
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
			if (mpSessionJoined) {
				net::MpClient::RespawnCountdownState rc{};
				bool haveRc = false;
				mpClient->getRespawnCountdownState(rc, haveRc);
				if (haveRc) {
					mpHaveRespawnCountdownState = true;
					mpHudServerTick = std::max(mpHudServerTick, rc.serverTick);
					if (rc.respawnAtServerTick == 0) {
						mpHaveRespawnCountdownState = false;
						mpRespawnAtServerTick = 0;
						mpRespawnSecondsPerServerTick = 0.0;
					} else {
						mpRespawnAtServerTick = rc.respawnAtServerTick;
						const std::uint64_t ticksLeft =
						    (rc.respawnAtServerTick > rc.serverTick)
						        ? (rc.respawnAtServerTick - rc.serverTick)
						        : 0u;
						if (ticksLeft > 0u && rc.wallSecondsRemaining > 1e-9) {
							mpRespawnSecondsPerServerTick =
							    rc.wallSecondsRemaining / static_cast<double>(ticksLeft);
						}
					}
				}
			}
			{
				net::JoinRejectReason rj = net::JoinRejectReason::NameInvalid;
				std::string rjDetail;
				if (mpClient->takeJoinReject(rj, rjDetail)) {
					const std::string msg = rjDetail.empty() ? "Join rejected" : rjDetail;
					setStatus(msg, 4.0);
					if (mpSim.has_value()) {
						mpSim->stop();
						mpSim.reset();
					}
					mpHaveRespawnCountdownState = false;
					mpRespawnAtServerTick = 0;
					mpRespawnSecondsPerServerTick = 0.0;
					mpClient.reset();
					mpJoinRequestSent = false;
					mpSessionJoined = false;
					multiplayer = false;
					menuPhase = MenuPhase::MpConnect;
					mpAddressEdit = mpHost + ":" + std::to_string(static_cast<unsigned>(mpPort));
				}
			}
			if (!mpJoinRequestSent && mpClient->isPeerConnected()) {
				mpClient->sendJoinRequest(playerName);
				mpJoinRequestSent = true;
			}
			std::uint64_t joinTick = 0;
			std::uint64_t joinGlobalPhysicsStep = 0;
			std::vector<sim::AuthoritativeBody> joinBodies;
			sim::BodyId joinOwn = 0;
			double joinTimeScale = 1.0;
			double joinPhysicsRealStep = net::kDefaultRealSecondsPerPhysicsStep;
			double joinShipThrustAccel = net::kDefaultShipThrustAccel;
			mpClient->takeJoinAccept(joinTick, joinGlobalPhysicsStep, joinBodies, joinOwn,
			                         joinTimeScale, joinPhysicsRealStep, joinShipThrustAccel);
			if (!joinBodies.empty()) {
				std::optional<std::pair<double, double>> ownShipCenter;
				if (joinOwn != 0) {
					const auto ownShipIt = std::find_if(
					    joinBodies.begin(), joinBodies.end(),
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
				mpSim->syncJoin(joinGlobalPhysicsStep, joinTimeScale, joinPhysicsRealStep,
				                joinShipThrustAccel, std::move(joinBodies));
				mpSim->start();
				if (mpOwnShipId != 0 && ownShipCenter.has_value()) {
					renderer.setWorldOriginForRendering(ownShipCenter->first,
					                                    ownShipCenter->second);
					renderer.setCameraWorldCenterDouble(ownShipCenter->first,
					                                    ownShipCenter->second);
				}
				trackedFollowId = mpOwnShipId;
				followViewOffset = {0.0, 0.0};
				mpHudServerTick = joinTick;
				mpNetPhysicsTimeScale =
				    (std::isfinite(joinTimeScale) && joinTimeScale > 0.0) ? joinTimeScale : 1.0;
				mpNetPrevInboundDropsTotal = mpClient->inboundPacketsDroppedTotal();
				mpNetPrevSimCapHitsTotal = mpSim->simPhysicsWakeCapHitsTotal();
				mpLastClientStressLog.reset();
				setStatus("Joined multiplayer session.", 2.5);
			}
			if (mpSessionJoined) {
				if (mpSim.has_value() && !mpClient->isConnected()) {
					mpSim->stop();
					mpSim.reset();
					mpSessionJoined = false;
					mpHaveRespawnCountdownState = false;
					mpRespawnAtServerTick = 0;
					mpRespawnSecondsPerServerTick = 0.0;
					mpShipReplica.clear();
					mpPrevOwnShipId = 0;
					mpLastResolvedOwnShipBodyId = 0;
					mpShipMouseAim = clientSettings.shipMouseAim;
					mpShipHeadingInited = false;
					mpShipThrustPercent = 100;
					mpShipDeltaVCurrentMps = 0.0f;
					mpShipDeltaVMaxMps = 0.0f;
					mpOwnShipStateGlobalPhysicsStep = 0;
					mpShellReadyGlobalPhysicsStep = 0;
					mpNetPhysicsTimeScale = 1.0;
					mpNetPrevInboundDropsTotal = 0;
					mpNetPrevSimCapHitsTotal = 0;
					mpLastClientStressLog.reset();
					mpOwnShipId = 0;
					mpClient.reset();
					mpJoinRequestSent = false;
					multiplayer = false;
					if (!cliQuickMultiplayer) {
						menuPhase = MenuPhase::Root;
					}
				} else {
					if (mpRenderFrame.has_value()) {
						resolveMpOwnShipAndCamera(mpRenderFrame->bodies);
					}
					while (true) {
						std::uint64_t mergeTick = 0;
						std::vector<std::pair<sim::BodyId, sim::BodyId>> netMerges;
						if (!mpClient->takeNextMergeRemaps(mergeTick, netMerges)) {
							break;
						}
						for (const auto& pr : netMerges) {
							mpShipReplica.erase(pr.first);
						}
						ui.selection.applyMergeRemap(netMerges);
						ui.traces.applyMergeRemap(netMerges);
						if (mpSim.has_value()) {
							mpSim->postMergeDeletes(std::move(netMerges));
						}
					}
					while (true) {
						std::uint64_t delTick = 0;
						std::uint64_t delStep = 0;
						std::vector<sim::BodyId> delIds;
						if (!mpClient->takeNextBodyDeleteBatch(delTick, delStep, delIds)) {
							break;
						}
						(void)delTick;
						(void)delStep;
						for (const sim::BodyId id : delIds) {
							mpShipReplica.erase(id);
							if (id == mpOwnShipId) {
								mpOwnShipId = 0;
								mpLastResolvedOwnShipBodyId = 0;
							}
						}
						ui.selection.applyBodyDeletes(delIds);
						ui.traces.applyBodyDeletes(delIds);
						if (mpSim.has_value()) {
							mpSim->postBodyDeleteBatch(std::move(delIds));
						}
					}

					std::vector<sim::AuthoritativeBody> mpAuthoritativeUpserts;
					bool hadAuthoritativeUpsert = false;
					mpClient->takeAuthoritativeUpserts(mpAuthoritativeUpserts,
					                                   hadAuthoritativeUpsert);
					if (hadAuthoritativeUpsert && mpSim.has_value()) {
						mpSim->postAuthoritativeUpserts(std::move(mpAuthoritativeUpserts));
					}

					mpClient->takeShipSamples(mpInboundShips);
					{
						std::uint64_t shipBatchTick = 0;
						if (mpClient->takeLatestShipBatchTick(shipBatchTick)) {
							mpHudServerTick = std::max(mpHudServerTick, shipBatchTick);
						}
					}
					for (const net::MpClient::ShipNetSample& s : mpInboundShips) {
						mpHudServerTick = std::max(mpHudServerTick, s.serverTick);
						MpShipReplica& rep = mpShipReplica[s.bodyId];
						rep.facing = s.facingRadians;
						rep.thrustForward = s.thrustForward;
						rep.thrustPercent = s.thrustPercent;
						rep.deltaVCurrentMps = s.deltaVCurrentMps;
						rep.deltaVMaxMps = s.deltaVMaxMps;
						rep.lastTick = s.serverTick;
						if (s.bodyId == mpOwnShipId) {
							mpShipDeltaVCurrentMps = s.deltaVCurrentMps;
							mpShipDeltaVMaxMps = s.deltaVMaxMps;
							mpOwnShipStateGlobalPhysicsStep = s.globalPhysicsStep;
							mpShellReadyGlobalPhysicsStep = s.shellReadyGlobalPhysicsStep;
						}
					}

					while (true) {
						std::uint64_t worldTick = 0;
						std::uint64_t worldStep = 0;
						std::vector<sim::AuthoritativeBody> worldBodies;
						if (!mpClient->takeNextWorldSnapshot(worldTick, worldStep, worldBodies)) {
							break;
						}
						mpHudServerTick = std::max(mpHudServerTick, worldTick);
						if (!mpSim.has_value()) {
							continue;
						}
						if (worldStep <= mpSim->lastConfirmedAuthorityStep()) {
							continue;
						}
						net::WorldSnapshotJob job;
						job.serverTick = worldTick;
						job.globalPhysicsStep = worldStep;
						job.bodies = std::move(worldBodies);
						mpSim->enqueueWorldSnapshot(std::move(job));
						mpSim->setServerPhysicsHeadTarget(worldStep);
					}

					if (mpSim.has_value()) {
						mpSim->syncReplicas(mpShipReplica);
					}
					if (mpSim.has_value() && mpOwnShipId != 0 &&
					    mpOwnShipStateGlobalPhysicsStep != 0u) {
						mpSim->setServerPhysicsHeadTarget(mpOwnShipStateGlobalPhysicsStep);
					}

					if (mpOwnShipId != mpPrevOwnShipId) {
						mpShipHeadingInited = false;
						if (mpOwnShipId == 0) {
							mpShipThrustPercent = 100;
							mpShipMouseAim = clientSettings.shipMouseAim;
							mpShipDeltaVCurrentMps = 0.0f;
							mpShipDeltaVMaxMps = 0.0f;
							mpOwnShipStateGlobalPhysicsStep = 0;
							mpShellReadyGlobalPhysicsStep = 0;
						}
						mpPrevOwnShipId = mpOwnShipId;
					}

					if (mpOwnShipId != 0) {
						if (windowKeyboardActive && !ui.menu.active()) {
							constexpr double kThrustAdjustPerSec = 80.0;
							if (keysHeld.down(sf::Keyboard::Key::LShift) ||
							    keysHeld.down(sf::Keyboard::Key::RShift)) {
								const int n =
								    static_cast<int>(mpShipThrustPercent) +
								    static_cast<int>(std::lround(kThrustAdjustPerSec * frameDt));
								mpShipThrustPercent =
								    static_cast<std::uint8_t>(std::min(100, std::max(0, n)));
							} else if (keysHeld.down(sf::Keyboard::Key::LControl) ||
							           keysHeld.down(sf::Keyboard::Key::RControl)) {
								const int n =
								    static_cast<int>(mpShipThrustPercent) -
								    static_cast<int>(std::lround(kThrustAdjustPerSec * frameDt));
								mpShipThrustPercent =
								    static_cast<std::uint8_t>(std::min(100, std::max(0, n)));
							}
						}

						const render::Renderer::WorldCoordsD mouseWorld =
						    renderer.screenToWorldD(sf::Mouse::getPosition(window));
						const auto shipIt = std::find_if(
						    bodies.begin(), bodies.end(),
						    [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
						const MpShipReplica* ownRep = nullptr;
						if (const auto it = mpShipReplica.find(mpOwnShipId);
						    it != mpShipReplica.end()) {
							ownRep = &it->second;
						}

						double facing = 0.0;
						if (mpShipMouseAim) {
							if (shipIt != bodies.end()) {
								facing =
								    std::atan2(mouseWorld.y - shipIt->y, mouseWorld.x - shipIt->x);
							} else if (ownRep != nullptr) {
								facing = static_cast<double>(ownRep->facing);
							}
							mpShipHeadingRadians = facing;
							mpShipHeadingInited = true;
						} else {
							if (!mpShipHeadingInited) {
								if (ownRep != nullptr) {
									mpShipHeadingRadians = static_cast<double>(ownRep->facing);
								} else if (shipIt != bodies.end()) {
									mpShipHeadingRadians = std::atan2(mouseWorld.y - shipIt->y,
									                                  mouseWorld.x - shipIt->x);
								}
								mpShipHeadingInited = true;
							}
							constexpr double kTurnRadPerSec = 2.85;
							if (windowKeyboardActive && keysHeld.down(sf::Keyboard::Key::A)) {
								mpShipHeadingRadians -= kTurnRadPerSec * frameDt;
							}
							if (windowKeyboardActive && keysHeld.down(sf::Keyboard::Key::D)) {
								mpShipHeadingRadians += kTurnRadPerSec * frameDt;
							}
							facing = mpShipHeadingRadians;
						}

						net::ClientInputPayload in{};
						in.seq = ++mpInputSeq;
						in.thrustForward = static_cast<std::uint8_t>(
						    windowKeyboardActive && (keysHeld.down(sf::Keyboard::Key::W) ||
						                             keysHeld.down(sf::Keyboard::Key::Up))
						        ? 1
						        : 0);
						in.facingRadians = static_cast<float>(facing);
						in.thrustPercent = mpShipThrustPercent;
						in.firePrimary = static_cast<std::uint8_t>(
						    windowKeyboardActive && !ui.menu.active() &&
						            keysHeld.down(sf::Keyboard::Key::Space)
						        ? 1
						        : 0);
						double shellAim = facing;
						double shellExtraSpeed = 0.0;
						const double shellTs =
						    (std::isfinite(mpNetPhysicsTimeScale) && mpNetPhysicsTimeScale > 0.0)
						        ? mpNetPhysicsTimeScale
						        : 1.0;
						if (shipIt != bodies.end()) {
							const double dx = mouseWorld.x - shipIt->x;
							const double dy = mouseWorld.y - shipIt->y;
							const double dist = std::sqrt(dx * dx + dy * dy);
							if (dist > 1e-6) {
								shellAim = std::atan2(dy, dx);
								// Aim distance is a per-real-second speed intent; packet carries
								// per sim-second magnitude (matches body velocity units).
								shellExtraSpeed = dist / shellTs;
							}
						}
						in.shellAimRadians = static_cast<float>(shellAim);
						in.shellExtraSpeed = static_cast<float>(shellExtraSpeed);
						mpClient->sendInput(in);
						// Own-ship `mpShipReplica` thrust/facing come from server `ShipState` only
						// (applied above) so client physics matches periodic world snapshots.
					}
				}
			}
		}

		if (!multiplayer && spSessionActive) {
			if (mpOwnShipId == 0) {
				std::vector<sim::BodySnapshot> resolveBodies;
				engine.copyBodies(resolveBodies);
				for (const sim::BodySnapshot& b : resolveBodies) {
					if (b.name == playerName) {
						mpOwnShipId = b.id;
						trackedFollowId = mpOwnShipId;
						followViewOffset = {0.0, 0.0};
						break;
					}
				}
			}
			if (mpOwnShipId != 0 && spFuelShipId != mpOwnShipId) {
				spFuelShipId = mpOwnShipId;
				spDeltaVCurrent = kSpMaxDeltaV;
				spWallSecondsSinceThrust = 0.0;
				spPendingThrustForward = false;
				(void)engine.takeAccumulatedSimulatedSecondsForShipFuel();
			}
			if (mpOwnShipId != 0) {
				const double simDtFuel = engine.takeAccumulatedSimulatedSecondsForShipFuel();
				bool hadEffectiveThrust = false;
				if (spPendingThrustForward && simDtFuel > 1e-12) {
					const double requestedDeltaV =
					    net::kDefaultShipThrustAccel * spPendingThrustScale * simDtFuel;
					const double allowedScale =
					    (requestedDeltaV > 1e-12)
					        ? std::clamp(spDeltaVCurrent / requestedDeltaV, 0.0, 1.0)
					        : 0.0;
					const double effectiveThrustScale = spPendingThrustScale * allowedScale;
					const double consumedDeltaV = requestedDeltaV * allowedScale;
					spDeltaVCurrent = std::max(0.0, spDeltaVCurrent - consumedDeltaV);
					if (effectiveThrustScale > 1e-9) {
						hadEffectiveThrust = true;
					}
				}
				if (hadEffectiveThrust) {
					spWallSecondsSinceThrust = 0.0;
				} else {
					spWallSecondsSinceThrust += frameDt;
					if (spWallSecondsSinceThrust >= kSpDeltaVRegenDelaySeconds) {
						spDeltaVCurrent = std::min(
						    kSpMaxDeltaV, spDeltaVCurrent + kSpDeltaVRegenPerRealSecond * frameDt);
					}
				}
				mpShipDeltaVCurrentMps = static_cast<float>(spDeltaVCurrent);
				mpShipDeltaVMaxMps = static_cast<float>(kSpMaxDeltaV);

				if (windowKeyboardActive && !ui.menu.active()) {
					std::vector<sim::BodySnapshot> soloBodies;
					engine.copyBodies(soloBodies);
					const auto shipIt = std::find_if(
					    soloBodies.begin(), soloBodies.end(),
					    [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
					if (shipIt != soloBodies.end()) {
						{
							constexpr double kThrustAdjustPerSec = 80.0;
							if (keysHeld.down(sf::Keyboard::Key::LShift) ||
							    keysHeld.down(sf::Keyboard::Key::RShift)) {
								const int n =
								    static_cast<int>(mpShipThrustPercent) +
								    static_cast<int>(std::lround(kThrustAdjustPerSec * frameDt));
								mpShipThrustPercent =
								    static_cast<std::uint8_t>(std::min(100, std::max(0, n)));
							} else if (keysHeld.down(sf::Keyboard::Key::LControl) ||
							           keysHeld.down(sf::Keyboard::Key::RControl)) {
								const int n =
								    static_cast<int>(mpShipThrustPercent) -
								    static_cast<int>(std::lround(kThrustAdjustPerSec * frameDt));
								mpShipThrustPercent =
								    static_cast<std::uint8_t>(std::min(100, std::max(0, n)));
							}
						}
						const sim::SimulationConfig liveCfg = engine.config();
						const double ts =
						    (std::isfinite(liveCfg.timeScale) && liveCfg.timeScale > 0.0)
						        ? liveCfg.timeScale
						        : 1.0;
						const double realStep =
						    (std::isfinite(liveCfg.fixedDtSeconds) && liveCfg.fixedDtSeconds > 0.0)
						        ? liveCfg.fixedDtSeconds
						        : net::kDefaultRealSecondsPerPhysicsStep;
						const render::Renderer::WorldCoordsD mouseWorld =
						    renderer.screenToWorldD(sf::Mouse::getPosition(window));
						double facing = 0.0;
						if (mpShipMouseAim) {
							facing = std::atan2(mouseWorld.y - shipIt->y, mouseWorld.x - shipIt->x);
							mpShipHeadingRadians = facing;
							mpShipHeadingInited = true;
						} else {
							if (!mpShipHeadingInited) {
								mpShipHeadingRadians =
								    std::atan2(mouseWorld.y - shipIt->y, mouseWorld.x - shipIt->x);
								mpShipHeadingInited = true;
							}
							constexpr double kTurnRadPerSec = 2.85;
							if (keysHeld.down(sf::Keyboard::Key::A)) {
								mpShipHeadingRadians -= kTurnRadPerSec * frameDt;
							}
							if (keysHeld.down(sf::Keyboard::Key::D)) {
								mpShipHeadingRadians += kTurnRadPerSec * frameDt;
							}
							facing = mpShipHeadingRadians;
						}
						const double ca = std::cos(facing);
						const double sa = std::sin(facing);
						const double thrustScale =
						    0.01 *
						    static_cast<double>(std::min<std::uint8_t>(mpShipThrustPercent, 100));
						double ax = 0.0;
						double ay = 0.0;
						const bool thrustOn = keysHeld.down(sf::Keyboard::Key::W) ||
						                      keysHeld.down(sf::Keyboard::Key::Up);
						if (thrustOn) {
							const double dtGovernor =
							    (simDtFuel > 1e-12)
							        ? simDtFuel
							        : std::max(1e-12, net::simulationDtFromTimeScale(ts, realStep));
							const double requestedDeltaVNow =
							    net::kDefaultShipThrustAccel * thrustScale * dtGovernor;
							const double allowedScaleNow =
							    (requestedDeltaVNow > 1e-12)
							        ? std::clamp(spDeltaVCurrent / requestedDeltaVNow, 0.0, 1.0)
							        : 0.0;
							const double effScale = thrustScale * allowedScaleNow;
							if (effScale > 1e-9) {
								ax += net::kDefaultShipThrustAccel * effScale * ca;
								ay += net::kDefaultShipThrustAccel * effScale * sa;
							}
						}
						engine.setShipThrustAccelWorld(mpOwnShipId, ax, ay);
						spPendingThrustForward = thrustOn;
						spPendingThrustScale = thrustScale;
						spShellCooldownWall = std::max(0.0, spShellCooldownWall - frameDt);
						if (keysHeld.down(sf::Keyboard::Key::Space) &&
						    spShellCooldownWall <= 1e-9) {
							double shellAim = facing;
							double shellExtraSpeed = 0.0;
							const double dx = mouseWorld.x - shipIt->x;
							const double dy = mouseWorld.y - shipIt->y;
							const double dist = std::sqrt(dx * dx + dy * dy);
							if (dist > 1e-6) {
								shellAim = std::atan2(dy, dx);
								shellExtraSpeed = dist / ts;
							}
							shellExtraSpeed = std::clamp(shellExtraSpeed, net::kShellSpeedMin,
							                             net::kShellSpeedMax);
							const double muzzleOffset = shipIt->radius + net::kShellRadius +
							                            net::kShellMuzzleSurfaceGapWorld;
							const std::uint64_t serial =
							    static_cast<std::uint64_t>(engine.simulationTimeSeconds() * 1e6) ^
							    static_cast<std::uint64_t>(mpOwnShipId);
							const std::string shellName =
							    "shell/local/" +
							    std::to_string(static_cast<unsigned long long>(serial));
							engine.queueSpawn(sim::SpawnCommand{
							    .x = shipIt->x + std::cos(shellAim) * muzzleOffset,
							    .y = shipIt->y + std::sin(shellAim) * muzzleOffset,
							    .vx = shipIt->vx + std::cos(shellAim) * shellExtraSpeed,
							    .vy = shipIt->vy + std::sin(shellAim) * shellExtraSpeed,
							    .mass = net::kShellMass,
							    .radius = net::kShellRadius,
							    .name = shellName,
							});
							spShellCooldownWall = net::kShellCooldownRealSeconds;
						}
					} else {
						engine.setShipThrustAccelWorld(mpOwnShipId, 0.0, 0.0);
						spPendingThrustForward = false;
					}
				} else {
					engine.setShipThrustAccelWorld(mpOwnShipId, 0.0, 0.0);
					spPendingThrustForward = false;
				}
			} else {
				(void)engine.takeAccumulatedSimulatedSecondsForShipFuel();
			}
		}

		mpRenderFrame.reset();
		if (mpSessionJoined && mpSim.has_value()) {
			net::MpClientRenderPublish pub;
			mpSim->copyLatestRenderPublish(pub);
			mpRenderFrame = std::move(pub);
			bodies = mpRenderFrame->bodies;
			resolveMpOwnShipAndCamera(bodies);
			const double pubTs = mpRenderFrame->config.timeScale;
			if (std::isfinite(pubTs) && pubTs > 0.0) {
				mpNetPhysicsTimeScale = pubTs;
			}
		}

		if (multiplayer && mpSessionJoined && mpClient.has_value() && mpSim.has_value()) {
			const std::uint64_t head = mpSim->clientPhysicsHead();
			const std::uint64_t auth = mpSim->lastConfirmedAuthorityStep();
			const std::uint64_t target = mpSim->serverPhysicsHeadTarget();
			const std::uint64_t behind = (auth > head) ? (auth - head) : 0u;
			const std::uint64_t leadVsTarget =
			    (target != 0u && head > target) ? (head - target) : 0u;
			const std::uint64_t ownStep = mpOwnShipStateGlobalPhysicsStep;
			const std::uint64_t ownVsAuth = (ownStep > auth) ? (ownStep - auth) : 0u;
			const std::uint64_t headToLeadCap =
			    (target != 0u && head < target + net::kMpLeadCapSteps)
			        ? (target + net::kMpLeadCapSteps - head)
			        : ((target == 0u && head < auth + net::kMpLeadCapSteps)
			               ? (auth + net::kMpLeadCapSteps - head)
			               : 0u);
			const bool headLeadCapReached =
			    (target != 0u && head >= target + net::kMpLeadCapSteps) ||
			    (target == 0u && head >= auth + net::kMpLeadCapSteps);
			const bool behindBad = behind >= net::kMpClientStressBehindAuthoritySteps;
			const bool leadBad =
			    leadVsTarget + net::kMpClientStressLeadNearCapSlackSteps >= net::kMpLeadCapSteps;
			const bool hitch = frameDt > net::kMpClientStressFrameHitchSeconds;
			std::uint64_t shipAhead = 0;
			if (mpOwnShipId != 0 && mpOwnShipStateGlobalPhysicsStep > head) {
				shipAhead = mpOwnShipStateGlobalPhysicsStep - head;
			}
			const bool shipBad = shipAhead >= net::kMpClientStressShipStateAheadSteps;
			const std::uint64_t dropsTotal = mpClient->inboundPacketsDroppedTotal();
			const std::uint64_t dropDelta = dropsTotal - mpNetPrevInboundDropsTotal;
			mpNetPrevInboundDropsTotal = dropsTotal;
			const bool dropsBad = dropDelta > 0u;
			const std::uint64_t capTotal = mpSim->simPhysicsWakeCapHitsTotal();
			const std::uint64_t capDelta = capTotal - mpNetPrevSimCapHitsTotal;
			mpNetPrevSimCapHitsTotal = capTotal;
			const bool capBad = capDelta > 0u;
			const std::size_t snapQ = mpSim->snapshotJobQueueDepth();
			const bool snapBad = snapQ >= net::kMpClientStressSnapshotJobQueueDepth;
			if (behindBad || leadBad || hitch || shipBad || dropsBad || capBad || snapBad) {
				const auto t = std::chrono::steady_clock::now();
				const bool cooldownOk =
				    !mpLastClientStressLog.has_value() ||
				    std::chrono::duration<double>(t - *mpLastClientStressLog).count() >=
				        net::kMpClientStressLogCooldownSeconds;
				if (cooldownOk) {
					std::fprintf(
					    stderr,
					    "[mp-stress] behind=%llu leadVsTgt=%llu frameMs=%.1f shipAhead=%llu "
					    "ownStep=%llu tgt=%llu auth=%llu head=%llu ownVsAuth=%llu "
					    "toLeadCap=%llu leadCapHit=%u rxDropDelta=%llu simCapDelta=%llu "
					    "snapQ=%zu reasons=",
					    static_cast<unsigned long long>(behind),
					    static_cast<unsigned long long>(leadVsTarget), frameDt * 1000.0,
					    static_cast<unsigned long long>(shipAhead),
					    static_cast<unsigned long long>(ownStep),
					    static_cast<unsigned long long>(target),
					    static_cast<unsigned long long>(auth),
					    static_cast<unsigned long long>(head),
					    static_cast<unsigned long long>(ownVsAuth),
					    static_cast<unsigned long long>(headToLeadCap),
					    headLeadCapReached ? 1u : 0u, static_cast<unsigned long long>(dropDelta),
					    static_cast<unsigned long long>(capDelta), snapQ);
					if (dropsBad) {
						std::fprintf(stderr, "drops;");
					}
					if (behindBad) {
						std::fprintf(stderr, "behind;");
					}
					if (snapBad) {
						std::fprintf(stderr, "snapQ;");
					}
					if (capBad) {
						std::fprintf(stderr, "simCap;");
					}
					if (leadBad) {
						std::fprintf(stderr, "leadCap;");
					}
					if (hitch) {
						std::fprintf(stderr, "frame;");
					}
					if (shipBad) {
						std::fprintf(stderr, "ship;");
					}
					std::fprintf(stderr, "\n");
					mpLastClientStressLog = t;
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
		for (const auto& pr : mergeRemapEvents) {
			mpShipReplica.erase(pr.first);
		}
		ui.selection.applyMergeRemap(mergeRemapEvents);
		ui.traces.applyMergeRemap(mergeRemapEvents);
		if (mpSessionJoined && mpSim.has_value()) {
			ui.selection.validateAgainstBodies(bodies);
		} else if (spSessionActive) {
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
		               ? ((std::isfinite(cfg.timeScale) && cfg.timeScale > 0.0) ? cfg.timeScale
		                                                                        : 1.0)
		               : ((engine.simulatedSecondsPerUpdate() > 0.0)
		                      ? engine.simulatedSecondsPerUpdate()
		                      : ((std::isfinite(cfg.timeScale) && cfg.timeScale > 0.0)
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
			shipPredictionStoppedOnEncounter = false;
			if (mpOwnShipId != 0) {
				const ui::PredictionPath shipPath = ui.predictor.predictForBody(
				    bodies, mpOwnShipId, cfg.gravitationalConstant, cfg.softeningEpsilon);
				const std::vector<sf::Vector2f>& shipPredicted = shipPath.points;
				if (!shipPredicted.empty()) {
					if (const auto it = std::find_if(
					        bodies.begin(), bodies.end(),
					        [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
					    it != bodies.end()) {
						const sf::Vector2f shipAnchor(static_cast<float>(it->x),
						                              static_cast<float>(it->y));
						shipPredictionOffsets.reserve(shipPredicted.size());
						if (selectedBody.has_value() && selectedBody->id != mpOwnShipId) {
							const ui::PredictionPath selectedPath = ui.predictor.predictForBody(
							    bodies, selectedBody->id, cfg.gravitationalConstant,
							    cfg.softeningEpsilon);
							const std::vector<sf::Vector2f>& selectedPredicted =
							    selectedPath.points;
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
								shipPredictionStoppedOnEncounter =
								    shipPath.stoppedOnEncounter && n == shipPredicted.size();
							} else {
								for (const sf::Vector2f& point : shipPredicted) {
									shipPredictionOffsets.push_back(point - shipAnchor);
								}
								shipPredictionStoppedOnEncounter = shipPath.stoppedOnEncounter;
							}
						} else {
							for (const sf::Vector2f& point : shipPredicted) {
								shipPredictionOffsets.push_back(point - shipAnchor);
							}
							shipPredictionStoppedOnEncounter = shipPath.stoppedOnEncounter;
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
		shellPrediction.clear();
		shellPredictionAnchorBodyId.reset();
		if (ui.showShellPrediction) {
			const sim::BodySnapshot* shipForShell = nullptr;
			if (mpOwnShipId != 0) {
				if (const auto it = std::find_if(
				        bodies.begin(), bodies.end(),
				        [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
				    it != bodies.end()) {
					shipForShell = &*it;
				}
			} else if (!multiplayer && selectedBody.has_value()) {
				shipForShell = &*selectedBody;
			}
			if (shipForShell != nullptr) {
				const render::Renderer::WorldCoordsD mouseWorld =
				    renderer.screenToWorldD(sf::Mouse::getPosition(window));
				const double dx = mouseWorld.x - shipForShell->x;
				const double dy = mouseWorld.y - shipForShell->y;
				double aim = 0.0;
				double extraSpeed = 0.0;
				if (dx * dx + dy * dy > 1e-12) {
					aim = std::atan2(dy, dx);
					extraSpeed = std::sqrt(dx * dx + dy * dy);
				} else {
					aim = 0.0;
				}
				const double ts =
				    (std::isfinite(cfg.timeScale) && cfg.timeScale > 0.0) ? cfg.timeScale : 1.0;
				const double speedDesiredSim = extraSpeed / ts;
				const double speedSim =
				    std::clamp(speedDesiredSim, net::kShellSpeedMin, net::kShellSpeedMax);
				const double muzzleOffset =
				    shipForShell->radius + net::kShellRadius + net::kShellMuzzleSurfaceGapWorld;
				const sim::SpawnCommand shellSpawn{
				    .x = shipForShell->x + std::cos(aim) * muzzleOffset,
				    .y = shipForShell->y + std::sin(aim) * muzzleOffset,
				    .vx = shipForShell->vx + std::cos(aim) * speedSim,
				    .vy = shipForShell->vy + std::sin(aim) * speedSim,
				    .mass = net::kShellMass,
				    .radius = net::kShellRadius,
				    .name = "shell/prediction",
				};
				sim::BodyId shellRelRefId = shipForShell->id;
				if (selectedBody.has_value()) {
					shellRelRefId = selectedBody->id;
				}
				const ui::PredictionPath shellPath = ui.predictor.predictSpawnRelativeToBody(
				    bodies, shellSpawn, shellRelRefId, cfg.gravitationalConstant,
				    cfg.softeningEpsilon);
				shellPrediction = shellPath.points;
				shellPredictionAnchorBodyId = shellRelRefId;
			}
		}

		std::vector<ui::MenuItem> menuItems{
		    {"trace.enabled", "Trails", ui.traces.settings().enabled ? "On" : "Off", false},
		    {"trace.relative", "Trails Frame", ui.traces.settings().relative ? "Relative" : "World",
		     false},
		    {"predict.ship", "Ship Prediction", ui.showShipSelfPrediction ? "On" : "Off", false},
		    {"predict.shell", "Shell Prediction", ui.showShellPrediction ? "On" : "Off", false},
		    {"follow.mode", "Follow Mode",
		     ui.followCameraMode == ui::UiState::FollowCameraMode::FollowOwnShip ? "Own ship"
		                                                                         : "Selected/0,0",
		     false},
		    {"action.resetview", "Reset View", "R", false},
		    {"action.fullscreen", "Fullscreen", "F11", false},
		};
		if (multiplayer && mpSessionJoined) {
			menuItems.push_back({"mp.disconnect", "Disconnect from server", "", false});
		}
		if (!multiplayer && spSessionActive) {
			menuItems.push_back({"sp.exit", "Exit to main menu", "", false});
		}
		ui.menu.setItems(menuItems);

		std::optional<float> playerFacing = std::nullopt;
		if (mpOwnShipId != 0) {
			if (const auto it = mpShipReplica.find(mpOwnShipId); it != mpShipReplica.end()) {
				playerFacing = it->second.facing;
			} else if (multiplayer && mpSessionJoined) {
				if (const auto it = std::find_if(
				        bodies.begin(), bodies.end(),
				        [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
				    it != bodies.end()) {
					const double vmag = std::hypot(it->vx, it->vy);
					playerFacing =
					    static_cast<float>(vmag > 1e-9 ? std::atan2(it->vy, it->vx) : 0.0);
				}
			} else if (!multiplayer && spSessionActive) {
				if (const auto it = std::find_if(
				        bodies.begin(), bodies.end(),
				        [&](const sim::BodySnapshot& b) { return b.id == mpOwnShipId; });
				    it != bodies.end()) {
					const double vmag = std::hypot(it->vx, it->vy);
					playerFacing =
					    static_cast<float>(vmag > 1e-9 ? std::atan2(it->vy, it->vx) : 0.0);
				}
			}
		}
		std::unordered_map<sim::BodyId, float> mpDrawShipFacings;
		const std::unordered_map<sim::BodyId, float>* mpDrawShipFacingsPtr = nullptr;
		if (multiplayer && mpSessionJoined) {
			mpDrawShipFacings.reserve(mpShipReplica.size());
			for (const auto& [id, rep] : mpShipReplica) {
				const auto bit =
				    std::find_if(bodies.begin(), bodies.end(),
				                 [&](const sim::BodySnapshot& b) { return b.id == id; });
				if (bit == bodies.end()) {
					continue;
				}
				if (bit->name == playerName) {
					continue;
				}
				if (id != mpOwnShipId && !bodyLooksLikeDefaultPlayerShip(*bit)) {
					continue;
				}
				mpDrawShipFacings[id] = rep.facing;
			}
			if (!mpDrawShipFacings.empty()) {
				mpDrawShipFacingsPtr = &mpDrawShipFacings;
			}
		}
		renderer.draw(bodies,
		              mpOwnShipId == 0 ? std::nullopt : std::optional<sim::BodyId>(mpOwnShipId),
		              playerFacing, mpDrawShipFacingsPtr,
		              (multiplayer && mpSessionJoined) ? std::optional<std::string_view>(playerName)
		                                               : std::nullopt);
		if (ui.showShellPrediction && shellPrediction.size() >= 2 &&
		    shellPredictionAnchorBodyId.has_value()) {
			// Points are time-synchronized (shell − reference body) from the predictor; reference
			// is the selected body when one exists, otherwise the firer. Re-anchor with that body's
			// current world position (trail-relative style: p − r(t) + anchor).
			const sim::BodySnapshot* shellAnchor = nullptr;
			if (const auto it = std::find_if(bodies.begin(), bodies.end(),
			                                 [&](const sim::BodySnapshot& b) {
				                                 return b.id == *shellPredictionAnchorBodyId;
			                                 });
			    it != bodies.end()) {
				shellAnchor = &*it;
			}
			if (shellAnchor != nullptr) {
				sf::VertexArray strip(sf::PrimitiveType::LineStrip, shellPrediction.size());
				for (std::size_t i = 0; i < shellPrediction.size(); ++i) {
					const double wx = static_cast<double>(shellPrediction[i].x) + shellAnchor->x;
					const double wy = static_cast<double>(shellPrediction[i].y) + shellAnchor->y;
					strip[i].position = renderer.worldToRenderLocal(wx, wy);
					strip[i].color = sf::Color(255, 120, 90, 140);
				}
				window.draw(strip);
			}
		}
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
		                           shipPredictionStoppedOnEncounter, std::nullopt, displayTimeRate,
		                           false, false, worldToRenderLocal, worldToPixel);
		overlay.drawWorldSelection(window, selectedBody, std::nullopt, ownShipBody, {}, false,
		                           std::nullopt, displayTimeRate, true, true, worldToRenderLocal,
		                           worldToPixel);
		overlay.drawWorldSelection(window, hoveredBody, selectedBody, selectedBody, {}, false,
		                           std::nullopt, displayTimeRate, false, true, worldToRenderLocal,
		                           worldToPixel);

		if (std::chrono::steady_clock::now() > statusUntil) {
			ui.statusMessage.clear();
		}
		std::vector<std::string> hudLines;
		if (multiplayer) {
			if (!mpSessionJoined) {
				hudLines.push_back("Multiplayer: " +
				                   (mpClient.has_value() && mpClient->isPeerConnected()
				                        ? std::string("joining ") + mpHost + ":" +
				                              std::to_string(static_cast<unsigned>(mpPort)) + "..."
				                        : std::string("connecting to ") + mpHost + ":" +
				                              std::to_string(static_cast<unsigned>(mpPort)) +
				                              "..."));
			} else {
				hudLines.push_back("Multiplayer: tick " + std::to_string(mpHudServerTick) +
				                   " | server: potato_gsim_server");
			}
		}
		hudLines.push_back("Bodies: " + std::to_string((mpSessionJoined && mpSim.has_value())
		                                                   ? bodies.size()
		                                                   : engine.bodyCount()));
		hudLines.push_back("Time speed: " + timeScaleDisplay);
		hudLines.push_back("Scale: " + scaleDisplay + " | Predict: " + predictWindowDisplay +
		                   " | FPS: " + std::to_string(round3(fps)));
		hudLines.push_back(
		    "Trails: " + std::string(ui.traces.settings().enabled ? "On" : "Off") + " (" +
		    std::string(ui.traces.settings().relative ? "Relative" : "World") + ")" +
		    " | Ship prediction: " + std::string(ui.showShipSelfPrediction ? "On" : "Off") +
		    " | Shell prediction: " + std::string(ui.showShellPrediction ? "On" : "Off"));
		if (!ui.statusMessage.empty()) {
			hudLines.push_back("Status: " + ui.statusMessage);
		}
		overlay.drawHudPanel(window, hudLines, ui.input.legendLines(ui.menu.active()), false);
		double respawnHudSeconds = 0.0;
		bool showRespawnHud = false;
		if (multiplayer && mpSessionJoined && mpHaveRespawnCountdownState &&
		    mpRespawnAtServerTick != 0 && mpRespawnSecondsPerServerTick > 0.0) {
			const std::uint64_t baseTick = mpHudServerTick;
			const std::uint64_t ticksLeft =
			    (mpRespawnAtServerTick > baseTick) ? (mpRespawnAtServerTick - baseTick) : 0u;
			respawnHudSeconds = static_cast<double>(ticksLeft) * mpRespawnSecondsPerServerTick;
			showRespawnHud = ticksLeft > 0u;
		}
		if (showRespawnHud && multiplayer && mpSessionJoined) {
			overlay.drawRespawnCountdownBanner(window, respawnHudSeconds);
		}
		if (mpOwnShipId != 0 && ((multiplayer && mpSessionJoined && mpSim.has_value()) ||
		                         (!multiplayer && spSessionActive))) {
			float shellReloadDisplaySec = 0.0f;
			if (multiplayer && mpSessionJoined && mpSim.has_value()) {
				shellReloadDisplaySec =
				    (mpShellReadyGlobalPhysicsStep > mpOwnShipStateGlobalPhysicsStep)
				        ? static_cast<float>(static_cast<double>(mpShellReadyGlobalPhysicsStep -
				                                                 mpOwnShipStateGlobalPhysicsStep) *
				                             mpSim->realSecondsPerPhysicsStep())
				        : 0.0f;
			} else {
				shellReloadDisplaySec = static_cast<float>(spShellCooldownWall);
			}
			overlay.drawShipThrustHud(window, static_cast<int>(mpShipThrustPercent),
			                          mpShipDeltaVCurrentMps, mpShipDeltaVMaxMps,
			                          shellReloadDisplaySec);
		}

		if (ui.menu.active()) {
			const sf::Font* menuFont = overlay.fontPtr();
			if (menuFont != nullptr) {
				ui.menu.draw(window, *menuFont);
			}
		}

		window.display();
	}

	if (mpSim.has_value()) {
		mpSim->stop();
		mpSim.reset();
	}
	mpClient.reset();
	enet_deinitialize();
	engine.stop();
	return 0;
}
