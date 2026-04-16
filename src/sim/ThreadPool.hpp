#pragma once

#include "sim/ChunkPolicy.hpp"

#include <atomic>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <mutex>
#include <thread>
#include <vector>

namespace sim {

class ThreadPool {
   public:
	ThreadPool();
	explicit ThreadPool(std::size_t workerCount);
	~ThreadPool();

	ThreadPool(const ThreadPool&) = delete;
	ThreadPool& operator=(const ThreadPool&) = delete;

	void resize(std::size_t workerCount);

	[[nodiscard]] std::size_t workerCount() const { return threads_.size(); }

	void parallelFor(std::size_t count,
	                 std::size_t chunkSize,
	                 ChunkPolicy policy,
	                 const std::function<void(std::size_t, std::size_t)>& fn);

   private:
	void startThreads(std::size_t workerCount);
	void stopThreads();
	void workerLoop(std::size_t workerId);

	std::vector<std::thread> threads_;
	mutable std::mutex mutex_;
	std::condition_variable cvJob_;
	std::condition_variable cvDone_;

	bool stop_ = false;

	std::uint64_t jobSequence_ = 0;
	std::size_t completedWorkers_ = 0;

	std::size_t taskCount_ = 0;
	std::size_t chunkSize_ = 1;
	std::size_t chunkCount_ = 0;
	std::size_t participantCount_ = 1;
	std::atomic<std::size_t> nextChunk_{0};
	ChunkPolicy chunkPolicy_ = ChunkPolicy::DynamicClaim;
	std::function<void(std::size_t, std::size_t)> taskFn_;
};

}  // namespace sim
