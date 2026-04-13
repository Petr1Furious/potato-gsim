#pragma once

#include "sim/BodyId.hpp"

#include <cstddef>
#include <mutex>
#include <utility>
#include <vector>

namespace sim {

struct SpawnCommand {
	double x = 0.0;
	double y = 0.0;
	double vx = 0.0;
	double vy = 0.0;
	double mass = 1.0;
	double radius = 1.0;
};

struct DeleteCommand {
	BodyId id = 0;
};

struct SimCommand {
	enum class Type { Spawn, Delete, ClearAll, ReplaceWorld };

	Type type = Type::Spawn;
	SpawnCommand spawn;
	DeleteCommand del;
	std::vector<SpawnCommand> replacementBodies;
};

class CommandQueue {
   public:
	void pushSpawn(const SpawnCommand& command) {
		std::lock_guard<std::mutex> lock(mutex_);
		pending_.push_back(SimCommand{
		    .type = SimCommand::Type::Spawn,
		    .spawn = command,
		    .del = {},
		});
	}

	void pushDelete(BodyId id) {
		std::lock_guard<std::mutex> lock(mutex_);
		pending_.push_back(SimCommand{
		    .type = SimCommand::Type::Delete,
		    .spawn = {},
		    .del = DeleteCommand{.id = id},
		    .replacementBodies = {},
		});
	}

	void pushClearAll() {
		std::lock_guard<std::mutex> lock(mutex_);
		pending_.push_back(SimCommand{
		    .type = SimCommand::Type::ClearAll,
		    .spawn = {},
		    .del = {},
		    .replacementBodies = {},
		});
	}

	void pushReplaceWorld(const std::vector<SpawnCommand>& bodies) {
		std::lock_guard<std::mutex> lock(mutex_);
		pending_.push_back(SimCommand{
		    .type = SimCommand::Type::ReplaceWorld,
		    .spawn = {},
		    .del = {},
		    .replacementBodies = bodies,
		});
	}

	void drainTo(std::vector<SimCommand>& out) {
		std::lock_guard<std::mutex> lock(mutex_);
		out.clear();
		out.swap(pending_);
	}

   private:
	std::mutex mutex_;
	std::vector<SimCommand> pending_;
};

}  // namespace sim
