#include "sim/SimulationEngine.hpp"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <numbers>
#include <random>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

namespace {

struct Candidate {
	std::string name;
	std::size_t collisionInterval = 1;
	std::size_t chunkSize = 0;
	std::size_t directSerialMaxBodies = 64;
	std::size_t directParallelMaxBodies = 2048;
	std::size_t workerCount = 0;
	std::size_t maxBodyCount = std::numeric_limits<std::size_t>::max();
	sim::ChunkPolicy chunkPolicy = sim::ChunkPolicy::DynamicClaim;
};

struct RunResult {
	double ups = 0.0;
	std::size_t finalBodies = 0;
	bool skipped = false;
	std::string skipReason;
};

struct AggregatedResult {
	double medianUps = 0.0;
	double iqrUps = 0.0;
	std::size_t medianFinalBodies = 0;
	bool skipped = false;
	std::string skipReason;
};

struct BenchmarkOptions {
	bool debugStepLog = false;
	std::vector<std::size_t> bodyCounts{10000, 50000, 100000, 200000};
	int repeats = 3;
};

std::vector<sim::SpawnCommand> makeDeterministicWorld(std::size_t count,
                                                      double centerX,
                                                      double centerY,
                                                      double spreadRadius) {
	constexpr double kEarthLikeDensity = 5514.0;
	std::vector<sim::SpawnCommand> bodies;
	bodies.reserve(count);
	const std::uint64_t seed =
	    0x9E3779B97F4A7C15ULL ^ (static_cast<std::uint64_t>(count) * 1315423911ULL);
	std::mt19937_64 rng(seed);
	std::uniform_real_distribution<double> angleDist(0.0, 2.0 * std::numbers::pi);
	std::uniform_real_distribution<double> radialDist(0.0, 1.0);
	std::uniform_real_distribution<double> massDist(1e21, 5e25);
	std::uniform_real_distribution<double> jitter(-1.5e9, 1.5e9);

	for (std::size_t i = 0; i < count; ++i) {
		const double a = angleDist(rng);
		const double r = std::sqrt(radialDist(rng)) * spreadRadius;
		const double x = centerX + std::cos(a) * r + jitter(rng);
		const double y = centerY + std::sin(a) * r + jitter(rng);
		const double mass = massDist(rng);
		const double radius =
		    std::cbrt((3.0 * mass) / (4.0 * std::numbers::pi * kEarthLikeDensity));
		bodies.push_back(sim::SpawnCommand{
		    .x = x,
		    .y = y,
		    .vx = 0.0,
		    .vy = 0.0,
		    .mass = mass,
		    .radius = radius,
		});
	}
	return bodies;
}

std::vector<std::size_t> parseBodyCountsCsv(const std::string& csv) {
	std::vector<std::size_t> out;
	std::stringstream ss(csv);
	std::string token;
	while (std::getline(ss, token, ',')) {
		if (token.empty()) {
			continue;
		}
		char* end = nullptr;
		const unsigned long long value = std::strtoull(token.c_str(), &end, 10);
		if (end == token.c_str() || *end != '\0' || value == 0ull) {
			continue;
		}
		out.push_back(static_cast<std::size_t>(value));
	}
	std::sort(out.begin(), out.end());
	out.erase(std::unique(out.begin(), out.end()), out.end());
	return out;
}

void printUsage(const char* argv0) {
	std::cout << "Usage: " << argv0
	          << " [--debug-step-log] [--bodies <csv>] [--bodies=<csv>] [--repeats <n>] [--help]\n"
	          << "  --debug-step-log   Enable per-step [sim-step] debug output.\n"
	          << "  --bodies <csv>     Comma-separated body counts, e.g. 100000 or 50000,100000.\n"
	          << "  --repeats <n>      Number of deterministic runs per candidate (default 3).\n";
}

bool parseArgs(int argc, char** argv, BenchmarkOptions& options) {
	for (int i = 1; i < argc; ++i) {
		const std::string arg = argv[i];
		if (arg == "--help" || arg == "-h") {
			printUsage(argv[0]);
			return false;
		}
		if (arg == "--debug-step-log") {
			options.debugStepLog = true;
			continue;
		}
		if (arg == "--bodies") {
			if (i + 1 >= argc) {
				std::cerr << "Missing value after --bodies\n";
				return false;
			}
			options.bodyCounts = parseBodyCountsCsv(argv[++i]);
			if (options.bodyCounts.empty()) {
				std::cerr << "Invalid --bodies list\n";
				return false;
			}
			continue;
		}
		if (arg.rfind("--bodies=", 0) == 0) {
			options.bodyCounts = parseBodyCountsCsv(arg.substr(9));
			if (options.bodyCounts.empty()) {
				std::cerr << "Invalid --bodies list\n";
				return false;
			}
			continue;
		}
		if (arg == "--repeats") {
			if (i + 1 >= argc) {
				std::cerr << "Missing value after --repeats\n";
				return false;
			}
			char* end = nullptr;
			const long parsed = std::strtol(argv[++i], &end, 10);
			if (end == argv[i] || *end != '\0' || parsed < 1 || parsed > 100) {
				std::cerr << "Invalid --repeats value\n";
				return false;
			}
			options.repeats = static_cast<int>(parsed);
			continue;
		}
		std::cerr << "Unknown argument: " << arg << '\n';
		printUsage(argv[0]);
		return false;
	}
	return true;
}

double medianOf(std::vector<double> values) {
	if (values.empty()) {
		return 0.0;
	}
	std::sort(values.begin(), values.end());
	const std::size_t mid = values.size() / 2;
	if ((values.size() & 1u) == 1u) {
		return values[mid];
	}
	return 0.5 * (values[mid - 1] + values[mid]);
}

double percentileOf(std::vector<double> values, double p) {
	if (values.empty()) {
		return 0.0;
	}
	std::sort(values.begin(), values.end());
	const double clamped = std::clamp(p, 0.0, 1.0);
	const double idx = clamped * static_cast<double>(values.size() - 1);
	const std::size_t lo = static_cast<std::size_t>(idx);
	const std::size_t hi = std::min(values.size() - 1, lo + 1);
	const double frac = idx - static_cast<double>(lo);
	return values[lo] * (1.0 - frac) + values[hi] * frac;
}

double measuredUpsFromSimulationTime(sim::SimulationEngine& engine,
                                     double simSecondsPerUpdate,
                                     std::chrono::milliseconds sampleWindow) {
	if (!(std::isfinite(simSecondsPerUpdate) && simSecondsPerUpdate > 0.0)) {
		return 0.0;
	}
	const auto realStart = std::chrono::steady_clock::now();
	const double simStart = engine.simulationTimeSeconds();
	std::this_thread::sleep_for(sampleWindow);
	const double simEnd = engine.simulationTimeSeconds();
	const auto realEnd = std::chrono::steady_clock::now();
	const double realSeconds = std::chrono::duration<double>(realEnd - realStart).count();
	if (!(std::isfinite(realSeconds) && realSeconds > 0.0)) {
		return 0.0;
	}
	const double simulatedDelta = std::max(0.0, simEnd - simStart);
	const double stepCount = simulatedDelta / simSecondsPerUpdate;
	return stepCount / realSeconds;
}

bool waitForBodies(sim::SimulationEngine& engine) {
	const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(15);
	while (std::chrono::steady_clock::now() < deadline) {
		if (engine.bodyCount() > 0) {
			return true;
		}
		std::this_thread::sleep_for(std::chrono::milliseconds(20));
	}
	return false;
}

RunResult runCase(std::size_t bodyCount,
                  const std::vector<sim::SpawnCommand>& world,
                  const Candidate& candidate,
                  bool debugStepLog) {
	if (bodyCount > candidate.maxBodyCount) {
		return RunResult{
		    .ups = 0.0,
		    .finalBodies = 0,
		    .skipped = true,
		    .skipReason = "n-too-large",
		};
	}

	sim::SimulationConfig cfg;
	cfg.mode = sim::SimulationMode::DeterministicFixedStep;
	cfg.timeScale = 3600.0;
	cfg.fixedDtSeconds = 1.0 / 240.0;
	cfg.gravitationalConstant = 6.67430e-11;
	cfg.softeningEpsilon = 1.0e6;
	cfg.barnesHutTheta = 0.6;
	cfg.collisionCellScale = 8.0;
	cfg.workerCount = candidate.workerCount;
	cfg.collisionStepInterval = candidate.collisionInterval;
	cfg.parallelChunkSize = candidate.chunkSize;
	cfg.directSerialMaxBodies = candidate.directSerialMaxBodies;
	cfg.directParallelMaxBodies = candidate.directParallelMaxBodies;
	cfg.chunkPolicy = candidate.chunkPolicy;

	sim::SimulationEngine engine(cfg);
	engine.start();
	engine.setDebugMetricsEnabled(false);
	engine.queueReplaceWorld(world);
	if (!waitForBodies(engine)) {
		const std::size_t finalCount = engine.bodyCount();
		engine.stop();
		return RunResult{.ups = 0.0, .finalBodies = finalCount};
	}
	engine.setDebugMetricsEnabled(debugStepLog);
	std::this_thread::sleep_for(std::chrono::milliseconds(1500));
	const double ups =
	    measuredUpsFromSimulationTime(engine, cfg.timeScale, std::chrono::milliseconds(2500));
	const std::size_t finalCount = engine.bodyCount();
	engine.stop();
	return RunResult{.ups = ups, .finalBodies = finalCount};
}

AggregatedResult runCaseRepeated(std::size_t bodyCount,
                                 const std::vector<sim::SpawnCommand>& world,
                                 const Candidate& candidate,
                                 bool debugStepLog,
                                 int repeats) {
	std::vector<double> upsSamples;
	std::vector<double> finalBodySamples;
	upsSamples.reserve(static_cast<std::size_t>(repeats));
	finalBodySamples.reserve(static_cast<std::size_t>(repeats));
	for (int i = 0; i < repeats; ++i) {
		const RunResult run = runCase(bodyCount, world, candidate, debugStepLog);
		if (run.skipped) {
			return AggregatedResult{
			    .skipped = true,
			    .skipReason = run.skipReason,
			};
		}
		upsSamples.push_back(run.ups);
		finalBodySamples.push_back(static_cast<double>(run.finalBodies));
	}
	const double q1 = percentileOf(upsSamples, 0.25);
	const double q3 = percentileOf(upsSamples, 0.75);
	return AggregatedResult{
	    .medianUps = medianOf(upsSamples),
	    .iqrUps = q3 - q1,
	    .medianFinalBodies = static_cast<std::size_t>(std::llround(medianOf(finalBodySamples))),
	};
}

}  // namespace

int main(int argc, char** argv) {
	BenchmarkOptions options;
	if (!parseArgs(argc, argv, options)) {
		return 0;
	}
	const std::vector<Candidate> candidates{
	    {"direct_serial", 1, 0, std::numeric_limits<std::size_t>::max(),
	     std::numeric_limits<std::size_t>::max(), 0, 2048, sim::ChunkPolicy::DynamicClaim},
	    {"direct_parallel", 1, 0, 0, std::numeric_limits<std::size_t>::max(), 0, 32768,
	     sim::ChunkPolicy::DynamicClaim},
	    {"barnes_hut_parallel", 1, 0, 0, 0, 0, std::numeric_limits<std::size_t>::max(),
	     sim::ChunkPolicy::DynamicClaim},
	};

	std::cout << "== potato_gsim throughput benchmark ==\n";
	std::cout << "Debug step log: " << (options.debugStepLog ? "on" : "off") << '\n';
	std::cout << "Repeats: " << options.repeats << '\n';
	std::cout << "Body counts: ";
	for (std::size_t i = 0; i < options.bodyCounts.size(); ++i) {
		std::cout << options.bodyCounts[i];
		if (i + 1 < options.bodyCounts.size()) {
			std::cout << ",";
		}
	}
	std::cout << '\n';
	std::cout << "BodiesInitial, Candidate, StepsPerSecMedian, StepsPerSecIQR, BodiesFinalMedian, "
	             "MergesMedian, Status\n";
	for (const std::size_t n : options.bodyCounts) {
		const double spread = 1.0e12 * std::sqrt(std::max(1.0, static_cast<double>(n) / 1.0e5));
		const std::vector<sim::SpawnCommand> world = makeDeterministicWorld(n, 0.0, 0.0, spread);
		for (const Candidate& candidate : candidates) {
			const AggregatedResult result =
			    runCaseRepeated(n, world, candidate, options.debugStepLog, options.repeats);
			if (result.skipped) {
				std::cout << n << ", " << candidate.name << ", SKIP, SKIP, 0, 0, "
				          << result.skipReason << '\n';
				continue;
			}
			const std::size_t merges =
			    n > result.medianFinalBodies ? (n - result.medianFinalBodies) : 0;
			std::cout << n << ", " << candidate.name << ", " << result.medianUps << ", "
			          << result.iqrUps << ", " << result.medianFinalBodies << ", " << merges
			          << ", ok\n";
		}
	}
	return 0;
}
