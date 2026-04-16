#include "sim/ThreadPool.hpp"

#include <algorithm>

namespace sim {

ThreadPool::ThreadPool() = default;

ThreadPool::ThreadPool(std::size_t workerCount) {
	startThreads(workerCount);
}

ThreadPool::~ThreadPool() {
	stopThreads();
}

void ThreadPool::resize(std::size_t workerCount) {
	stopThreads();
	startThreads(workerCount);
}

void ThreadPool::startThreads(std::size_t workerCount) {
	stop_ = false;
	threads_.reserve(workerCount);
	for (std::size_t i = 0; i < workerCount; ++i) {
		threads_.emplace_back([this, i] { workerLoop(i); });
	}
}

void ThreadPool::stopThreads() {
	{
		std::lock_guard<std::mutex> lock(mutex_);
		stop_ = true;
		++jobSequence_;
	}
	cvJob_.notify_all();

	for (std::thread& thread : threads_) {
		if (thread.joinable()) {
			thread.join();
		}
	}
	threads_.clear();

	stop_ = false;
	completedWorkers_ = 0;
}

void ThreadPool::parallelFor(std::size_t count,
                             std::size_t chunkSize,
                             ChunkPolicy policy,
                             const std::function<void(std::size_t, std::size_t)>& fn) {
	if (count == 0) {
		return;
	}

	if (chunkSize == 0) {
		chunkSize = 1;
	}

	if (threads_.empty()) {
		fn(0, count);
		return;
	}
	const std::size_t localChunkCount = (count + chunkSize - 1) / chunkSize;
	if (localChunkCount <= 1) {
		fn(0, count);
		return;
	}

	std::uint64_t localSeq = 0;
	{
		std::lock_guard<std::mutex> lock(mutex_);
		taskCount_ = count;
		chunkSize_ = chunkSize;
		chunkCount_ = localChunkCount;
		participantCount_ = threads_.size() + 1;
		chunkPolicy_ = policy;
		nextChunk_.store(0, std::memory_order_relaxed);
		taskFn_ = fn;
		completedWorkers_ = 0;
		++jobSequence_;
		localSeq = jobSequence_;
	}

	cvJob_.notify_all();

	if (policy == ChunkPolicy::DynamicClaim) {
		while (true) {
			const std::size_t chunk = nextChunk_.fetch_add(1, std::memory_order_relaxed);
			if (chunk >= chunkCount_) {
				break;
			}
			const std::size_t begin = chunk * chunkSize_;
			const std::size_t end = std::min(taskCount_, begin + chunkSize_);
			fn(begin, end);
		}
	} else {
		const std::size_t mainWorkerId = threads_.size();
		for (std::size_t chunk = mainWorkerId; chunk < chunkCount_; chunk += participantCount_) {
			const std::size_t begin = chunk * chunkSize_;
			const std::size_t end = std::min(taskCount_, begin + chunkSize_);
			fn(begin, end);
		}
	}

	std::unique_lock<std::mutex> lock(mutex_);
	cvDone_.wait(lock, [this, localSeq] {
		return stop_ || completedWorkers_ == threads_.size() || jobSequence_ != localSeq;
	});
}

void ThreadPool::workerLoop(std::size_t workerId) {
	std::uint64_t observedSeq = 0;
	while (true) {
		std::function<void(std::size_t, std::size_t)> fn;
		std::size_t count = 0;
		std::size_t chunkSize = 1;
		std::size_t chunkCount = 0;
		std::size_t participants = 1;
		ChunkPolicy policy = ChunkPolicy::DynamicClaim;

		{
			std::unique_lock<std::mutex> lock(mutex_);
			cvJob_.wait(lock, [this, observedSeq] { return stop_ || jobSequence_ != observedSeq; });
			if (stop_) {
				return;
			}
			observedSeq = jobSequence_;
			fn = taskFn_;
			count = taskCount_;
			chunkSize = chunkSize_;
			chunkCount = chunkCount_;
			participants = participantCount_;
			policy = chunkPolicy_;
		}

		if (policy == ChunkPolicy::DynamicClaim) {
			while (true) {
				const std::size_t chunk = nextChunk_.fetch_add(1, std::memory_order_relaxed);
				if (chunk >= chunkCount) {
					break;
				}
				const std::size_t begin = chunk * chunkSize;
				const std::size_t end = std::min(count, begin + chunkSize);
				fn(begin, end);
			}
		} else {
			for (std::size_t chunk = workerId; chunk < chunkCount; chunk += participants) {
				const std::size_t begin = chunk * chunkSize;
				const std::size_t end = std::min(count, begin + chunkSize);
				fn(begin, end);
			}
		}

		{
			std::lock_guard<std::mutex> lock(mutex_);
			++completedWorkers_;
			if (completedWorkers_ == threads_.size()) {
				cvDone_.notify_one();
			}
		}
	}
}

}  // namespace sim
