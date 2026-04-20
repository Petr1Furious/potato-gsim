#pragma once

#include "sim/BodyId.hpp"

#include <cstddef>
#include <mutex>
#include <string>
#include <vector>

namespace sim {

struct SpawnCommand {
	double x = 0.0;
	double y = 0.0;
	double vx = 0.0;
	double vy = 0.0;
	double mass = 1.0;
	double radius = 1.0;
	std::string name;
};

struct DeleteCommand {
	BodyId id = 0;
};

struct RenameCommand {
	BodyId id = 0;
	std::string name;
};

struct AuthoritativeBody {
	BodyId id = 0;
	double x = 0.0;
	double y = 0.0;
	double vx = 0.0;
	double vy = 0.0;
	double mass = 1.0;
	double radius = 1.0;
	std::string name;
};

struct BodyDynamicsPatch {
	BodyId id = 0;
	double x = 0.0;
	double y = 0.0;
	double vx = 0.0;
	double vy = 0.0;
};

struct SimCommand {
	enum class Type {
		Spawn,
		Delete,
		Rename,
		ClearAll,
		ReplaceWorld,
		ApplyAuthoritativeSnapshot,
		PatchDynamics,
		UpsertAuthoritative
	};

	Type type = Type::Spawn;
	SpawnCommand spawn;
	DeleteCommand del;
	RenameCommand rename;
	std::vector<SpawnCommand> replacementBodies;
	std::vector<AuthoritativeBody> authoritativeBodies;
	std::vector<BodyDynamicsPatch> dynamicPatches{};
	std::vector<AuthoritativeBody> upsertBodies{};
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

	void pushRename(BodyId id, const std::string& name) {
		std::lock_guard<std::mutex> lock(mutex_);
		pending_.push_back(SimCommand{
		    .type = SimCommand::Type::Rename,
		    .rename =
		        RenameCommand{
		            .id = id,
		            .name = name,
		        },
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

	void pushApplyAuthoritativeSnapshot(std::vector<AuthoritativeBody> bodies) {
		std::lock_guard<std::mutex> lock(mutex_);
		pending_.push_back(SimCommand{
		    .type = SimCommand::Type::ApplyAuthoritativeSnapshot,
		    .spawn = {},
		    .del = {},
		    .replacementBodies = {},
		    .authoritativeBodies = std::move(bodies),
		    .dynamicPatches = {},
		});
	}

	void pushPatchDynamics(std::vector<BodyDynamicsPatch> patches) {
		if (patches.empty()) {
			return;
		}
		std::lock_guard<std::mutex> lock(mutex_);
		pending_.push_back(SimCommand{
		    .type = SimCommand::Type::PatchDynamics,
		    .spawn = {},
		    .del = {},
		    .replacementBodies = {},
		    .authoritativeBodies = {},
		    .dynamicPatches = std::move(patches),
		});
	}

	void pushUpsertAuthoritative(std::vector<AuthoritativeBody> bodies) {
		if (bodies.empty()) {
			return;
		}
		std::lock_guard<std::mutex> lock(mutex_);
		pending_.push_back(SimCommand{
		    .type = SimCommand::Type::UpsertAuthoritative,
		    .spawn = {},
		    .del = {},
		    .replacementBodies = {},
		    .authoritativeBodies = {},
		    .dynamicPatches = {},
		    .upsertBodies = std::move(bodies),
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
