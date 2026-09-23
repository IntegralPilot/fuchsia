// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_RUNTIME_THREAD_POOL_H_
#define SRC_DEVICES_BIN_DRIVER_RUNTIME_THREAD_POOL_H_

#include <lib/async-loop/cpp/loop.h>
#include <zircon/types.h>

#include <string_view>

#include <fbl/intrusive_wavl_tree.h>

#include "dispatcher.h"
#include "dispatcher_internals.h"

namespace driver_runtime {

class ThreadPool : public fbl::WAVLTreeContainable<std::unique_ptr<ThreadPool>> {
 public:
  // The default pool is for the dispatchers with no specified scheduler role.
  static constexpr std::string_view kNoSchedulerRole = "";

  explicit ThreadPool(std::string_view scheduler_role = kNoSchedulerRole, bool unmanaged = false)
      : scheduler_role_(scheduler_role),
        is_unmanaged_(unmanaged),
        config_(MakeConfig(this, scheduler_role)),
        loop_(&config_) {}

  // Required to instantiate fbl::DefaultKeyedObjectTraits.
  std::string GetKey() const { return scheduler_role_; }

  // Starts a new thread on the thread pool unconditionally.
  zx_status_t AddThread();

  // Decrements the number of required threads. Currently this doesn't spin down the extra thread
  // but for now that is ok since more often than not it can be used by another dispatcher on the
  // thread-pool. If it is not used, there will simply be one more thread than needed.
  // TODO(https://fxbug.dev/326266527): Use a timer to spin down un-necessary thread.
  zx_status_t OnDispatcherSealed();

  // Updates the number of threads needed in the thread pool. Starts a new thread if needed.
  zx_status_t OnDispatcherAdded(Dispatcher& dispatcher);
  // Updates the number of threads needed in the thread pool.
  void OnDispatcherRemoved(Dispatcher& dispatcher);
  // Requests the profile provider set the role profile.
  zx_status_t SetRoleProfile();

  // Resets to 0 threads.
  // Must only be called when there are no outstanding dispatchers.
  // Must not be called from within a driver_runtime managed thread as that will result in a
  // deadlock.
  void Reset();

  // Stores |irq| which has been unbound.
  // This is avoid destroying the irq wrapper immediately after unbinding, as it's possible
  // another thread in the thread pool has already pulled an irq packet
  // from the port and may attempt to call the irq handler.
  void CacheUnboundIrq(std::unique_ptr<AsyncIrq> irq);

  // Updates the thread tracking and checks whether to garbage collect the current generation of
  // irqs.
  void OnThreadWakeup();

  // Returns the number of threads that have been started on |loop_|.
  uint32_t num_threads() const {
    fbl::AutoLock al(&lock_);
    return num_threads_;
  }

  uint32_t thread_limit() const {
    fbl::AutoLock al(&lock_);
    return thread_limit_;
  }

  zx_status_t set_thread_limit(uint32_t max_threads) {
    fbl::AutoLock al(&lock_);
    if (max_threads < num_threads_) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    thread_limit_ = max_threads;
    return ZX_OK;
  }

  uint32_t scheduler_role_options() const {
    fbl::AutoLock al(&lock_);
    return scheduler_role_options_;
  }

  zx_status_t set_scheduler_role_options(uint32_t options) {
    // reject unknown options
    if ((options & ~FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS) != 0) {
      return ZX_ERR_INVALID_ARGS;
    }
    fbl::AutoLock al(&lock_);
    // don't allow setting no-sync-calls if there's already an allow-sync-calls dispatcher.
    if ((options & FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS) && allow_sync_call_dispatchers_ != 0) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    scheduler_role_options_ = options;
    return ZX_OK;
  }

  bool ScanThreadsForStalls();

  uint32_t num_dispatchers() const {
    fbl::AutoLock al(&lock_);
    return num_dispatchers_;
  }

  bool is_unmanaged() const { return is_unmanaged_; }

  std::string_view scheduler_role() const { return scheduler_role_; }
  async::Loop* loop() { return &loop_; }

  static constexpr uint32_t kDefaultThreadLimit = 20;

 private:
  // This stores irqs to avoid destroying them immediately after unbinding.
  // Even though unbinding an irq will clear all irq packets on a port,
  // it's possible another thread in the thread pool has already pulled an irq packet
  // from the port and may attempt to call the irq handler.
  //
  // It is safe to destroy a cached irq once we can determine that all threads
  // have woken up at least once since the irq was unbound.
  class CachedIrqs {
   public:
    // Adds an unbound irq to the cached irqs.
    void AddIrqLocked(std::unique_ptr<AsyncIrq> irq) __TA_REQUIRES(&lock_);

    void NewThreadWakeupLocked(uint32_t total_number_threads) __TA_REQUIRES(&lock_);

    // The coordinator can compare the current generation id to a thread's stored generation id to
    // see if the thread wakeup has not yet been tracked, in which case |NewThreadWakeupLocked|
    // should be called.
    uint32_t cur_generation_id() { return cur_generation_id_.load(); }

   private:
    using List = fbl::DoublyLinkedList<std::unique_ptr<AsyncIrq>, fbl::DefaultObjectTag,
                                       fbl::SizeOrder::Constant>;

    void IncrementGenerationId() __TA_REQUIRES(&lock_) {
      if (cur_generation_id_.fetch_add(1) == UINT32_MAX) {
        // |fetch_add| returns the value before adding. Avoid using 0 for a new generation id,
        // since new threads may be spawned with default generation id 0.
        cur_generation_id_++;
      }
    }

    // The current generation of cached irqs to be garbage collected once all threads wakeup.
    List cur_generation_ __TA_GUARDED(&lock_);
    // These are the irqs that were unbound after we already tracked a thread wakeup for the
    // current generation.
    List next_generation_ __TA_GUARDED(&lock_);

    // The number of threads that have woken up since the irqs in the |cur_generation_| list was
    // populated.
    uint32_t threads_wakeup_count_ __TA_GUARDED(&lock_) = 0;

    // This is not locked for reads, so that threads do not need to deal with lock contention if
    // there are no cached irqs.
    std::atomic<uint32_t> cur_generation_id_ = 0;
  };

  static constexpr async_loop_config_t MakeConfig(ThreadPool* self,
                                                  std::string_view scheduler_role) {
    async_loop_config_t config = kAsyncLoopConfigNeverAttachToThread;
    config.irq_support = true;
    config.data = self;
    // Add a thread wakeup handler.
    config.prologue = [](async_loop_t* loop, void* data) {
      ThreadPool* thread_pool = static_cast<ThreadPool*>(data);
      thread_pool->ThreadWakeupPrologue();
    };
    config.epilogue = [](async_loop_t* loop, void* data) {
      ThreadPool* thread_pool = static_cast<ThreadPool*>(data);
      thread_pool->ThreadWakeupEpilogue();
    };
    return config;
  }

  // Function that runs for every thread wakeup before any handler is called.
  void ThreadWakeupPrologue();

  // Function that runs for every thread wakeup after any handler is called.
  void ThreadWakeupEpilogue();

  // The actual current limit on the number of threads we'll spawn, based on the number and
  // types of dispatchers as well as the user-settable limit.
  // The heuristic is basically, whichever is the lowest of:
  // - up to one thread for every dispatcher that allows sync calls, plus one thread for
  // all other dispatchers (which should never block).
  // - one thread for every dispatcher.
  // - the user-settable |thread_limit_|.
  uint32_t MaxThreadsLocked() const __TA_REQUIRES(&lock_) {
    return std::min({allow_sync_call_dispatchers_ + 1, num_dispatchers_, thread_limit_});
  }

  // Starts a new thread on the thread pool unconditionally. The caller should check if
  // we're not at maximum with |MaxThreadsLocked|.
  zx_status_t AddThreadLocked() __TA_REQUIRES(&lock_);

  std::string scheduler_role_;

  mutable fbl::Mutex lock_;
  // Options that affect the kinds of dispatchers that can be created on this thread pool.
  // This is guarded by the lock because we have to prevent the precondition specified by
  // these options from being violated while they're being set.
  uint32_t scheduler_role_options_ __TA_GUARDED(&lock_) = 0;
  // Tracks the number of dispatchers which have sync calls allowed. We want to only spawn enough
  // threads needed so that every sync call dispatcher can have a thread to itself, at most.
  // See |MaxThreadsLocked| for more info.
  uint32_t allow_sync_call_dispatchers_ __TA_GUARDED(&lock_) = 0;
  // Tracks the number of threads we've spawned via |loop_|.
  uint32_t num_threads_ __TA_GUARDED(&lock_) = 0;
  // Total number of threads we will spawn.
  // TODO(https://fxbug.dev/42085539): We are clamping number_threads_ to 10 to avoid spawning too
  // many threads. Technically this can result in a deadlock scenario in a very complex driver
  // host. We need better support for dynamically starting threads as necessary.
  uint32_t thread_limit_ __TA_GUARDED(&lock_) = kDefaultThreadLimit;
  // A unique_ptr to each active thread's task entry time slot, used to tell when we've run
  // out of un-stalled threads and should spawn another.
  std::vector<std::pair<zx_koid_t, std::atomic_int64_t*>> thread_entry_time_slots_
      __TA_GUARDED(&lock_);
  // True if we've already attempted to spawn a new thread in response to the current thread
  // stall. This prevents us from constantly warning when we're at max threads and there's a
  // persistent stall.
  bool stalled_ __TA_GUARDED(&lock_) = false;

  // Total number of threads which have entered a driver. When this number matches num_threads_,
  // we start polling.
  std::atomic<uint32_t> threads_entered_ = 0;

  uint32_t num_dispatchers_ __TA_GUARDED(&lock_) = 0;

  bool is_unmanaged_;

  // Stores unbound irqs which will be garbage collected at a later time.
  CachedIrqs cached_irqs_;

  async_loop_config_t config_;
  // |loop_| must be declared last, to ensure that the loop shuts down before
  // other members are destructed.
  async::Loop loop_;
};

}  // namespace driver_runtime

#endif  // SRC_DEVICES_BIN_DRIVER_RUNTIME_THREAD_POOL_H_
