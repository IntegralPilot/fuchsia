// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_COORDINATOR_H_
#define SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_COORDINATOR_H_

#include "dispatcher.h"
#include "thread_pool.h"
#include "token_manager.h"

namespace driver_runtime {

namespace {

constexpr uint64_t kStallTimeMs = 350;
constexpr uint64_t kStaleTimeMs = kStallTimeMs * 5;

}  // namespace

// Coordinator for all dispatchers in a process.
class DispatcherCoordinator {
 public:
  // We default to no threads, and start additional threads when blocking dispatchers are created.
  DispatcherCoordinator() {
    auto thread_pool = default_thread_pool();
    token_manager_.SetGlobalDispatcher(thread_pool->loop()->dispatcher());
  }

  static void DestroyAllDispatchers();
  static void WaitUntilDispatchersIdle();
  static void WaitUntilDispatchersDestroyed();
  static zx_status_t TestingRun(zx::time deadline, bool once);
  static zx_status_t TestingRunUntilIdle();
  static void TestingQuit();
  static zx_status_t TestingResetQuit();
  static zx_status_t ShutdownDispatchersAsync(const void* driver,
                                              fdf_env_driver_shutdown_observer_t* observer);
  static zx_status_t SuspendDispatchersAsync(const void* driver,
                                             fdf_env_suspend_completer_t* completer);
  static void ResumeDispatchers(const void* driver);
  static void RegisterResumeRequester(const void* driver, fdf_env_resume_requester_t* requester);

  // Implementation of fdf_token_*.
  static zx_status_t TokenRegister(zx_handle_t token, fdf_dispatcher_t* dispatcher,
                                   fdf_token_t* handler);
  static zx_status_t TokenReceive(zx_handle_t token, fdf_handle_t* handle);
  static zx_status_t TokenTransfer(zx_handle_t token, fdf_handle_t channel);

  // Implementation of fdf_env_*.
  static uint32_t GetThreadLimit(std::string_view scheduler_role);
  static zx_status_t SetThreadLimit(std::string_view scheduler_role, uint32_t max_threads);
  static uint32_t GetSchedulerRoleOpts(std::string_view scheduler_role);
  static zx_status_t SetSchedulerRoleOpts(std::string_view scheduler_role, uint32_t opts);
  zx_duration_mono_t ScanThreadsForStalls();
  void RegisterStallScanner(fdf_env_stall_scanner_t* stall_scanner);
  void TriggerStallScanner();

  // Returns ZX_OK if |dispatcher| was added successfully.
  // Returns ZX_ERR_BAD_STATE if the driver is currently shutting down.
  zx_status_t AddDispatcher(fbl::RefPtr<Dispatcher> dispatcher, std::string_view scheduler_role,
                            std::unique_ptr<EventWaiter> event_waiter);
  zx_status_t AddUnmanagedDispatcher(fbl::RefPtr<Dispatcher> dispatcher,
                                     std::unique_ptr<EventWaiter> event_waiter);
  // Notifies the dispatcher coordinator that a dispatcher has completed shutdown.
  // |dispatcher_shutdown_observer| is the observer to call.
  void NotifyDispatcherShutdown(driver_runtime::Dispatcher& dispatcher,
                                fdf_dispatcher_shutdown_observer_t* dispatcher_shutdown_observer);
  void RemoveDispatcher(Dispatcher& dispatcher);
  static zx_status_t Start(uint32_t options);
  static void EnvReset();

  bool AreAllDriversDestroyedLocked() __TA_REQUIRES(&lock_) {
    return (drivers_.size() == 0) && (num_notify_shutdown_threads_ == 0);
  }

  // Resets to 0 threads.
  // Must only be called when there are no outstanding dispatchers.
  // Must not be called from within a driver_runtime managed thread as that will result in a
  // deadlock.
  void Reset();

  // Returns the thread pool for |scheduler_role| if it exists.
  std::optional<ThreadPool*> GetThreadPool(std::string_view scheduler_role);
  // Returns the thread pool for |scheduler_role|.
  // If the thread pool does not exists, creates the thread pool and starts the initial thread.
  zx::result<ThreadPool*> GetOrCreateThreadPool(std::string_view scheduler_role);
  // This will schedule the thread pool to be deleted on a thread on the default thread pool.
  void DestroyThreadPool(ThreadPool* thread_pool) __TA_REQUIRES(&lock_);

  ThreadPool* default_thread_pool() { return &default_thread_pool_; }

  // Returns the unmanaged thread pool. Creates it first if it doesn't exist.
  ThreadPool* GetOrCreateUnmanagedThreadPool() {
    if (!unmanaged_thread_pool_.has_value()) {
      unmanaged_thread_pool_.emplace(ThreadPool::kNoSchedulerRole, /*unmanaged*/ true);
    }

    return &unmanaged_thread_pool_.value();
  }

  static bool dynamic_thread_spawning() { return options_ & FDF_ENV_DYNAMIC_THREAD_SPAWNING; }

  static bool enforce_allowed_scheduler_roles() {
    return options_ & FDF_ENV_ENFORCE_ALLOWED_SCHEDULER_ROLES;
  }

 private:
  // Tracks the dispatchers owned by a driver.
  class DriverState : public fbl::RefCounted<DriverState>,
                      public fbl::WAVLTreeContainable<fbl::RefPtr<DriverState>> {
   public:
    using DriverShutdownCallback = fit::inline_callback<void(void), sizeof(void*) * 3>;

    explicit DriverState(const void* driver) : driver_(driver) {}

    void SetResumeRequester(fdf_env_resume_requester_t* r) { resume_requester_ = r; }
    fdf_env_resume_requester_t* GetResumeRequester() const { return resume_requester_; }

    // Required to instantiate fbl::DefaultKeyedObjectTraits.
    const void* GetKey() const { return driver_; }

    void AddDispatcher(fbl::RefPtr<driver_runtime::Dispatcher> dispatcher) {
      if (initial_dispatcher_ == nullptr) {
        initial_dispatcher_ = dispatcher;
      }
      dispatchers_.push_back(std::move(dispatcher));
    }
    void SetDispatcherShutdown(driver_runtime::Dispatcher& dispatcher) {
      shutdown_dispatchers_.push_back(dispatchers_.erase(dispatcher));
    }
    void RemoveDispatcher(driver_runtime::Dispatcher& dispatcher) {
      shutdown_dispatchers_.erase(dispatcher);
    }

    // Appends reference pointers of the driver's dispatchers to the |dispatchers| vector.
    void GetDispatchers(std::vector<fbl::RefPtr<driver_runtime::Dispatcher>>& dispatchers) {
      dispatchers.reserve(dispatchers.size() + dispatchers_.size_slow());
      for (auto& dispatcher : dispatchers_) {
        dispatchers.emplace_back(fbl::RefPtr<Dispatcher>(&dispatcher));
      }
    }

    // Appends reference pointers of the driver's shutdown dispatchers to the |dispatchers| vector.
    void GetShutdownDispatchers(std::vector<fbl::RefPtr<driver_runtime::Dispatcher>>& dispatchers) {
      for (auto& dispatcher : shutdown_dispatchers_) {
        dispatchers.emplace_back(fbl::RefPtr<Dispatcher>(&dispatcher));
      }
    }

    // Sets the driver as shutting down, and the callback which will be invoked once
    // shutting down the driver's dispatchers completes.
    zx_status_t SetDriverShuttingDown(DriverShutdownCallback callback) {
      if (shutdown_callback_ || driver_shutting_down_) {
        // Currently we only support one observer at a time.
        return ZX_ERR_BAD_STATE;
      }
      driver_shutting_down_ = true;
      shutdown_callback_ = std::move(callback);
      return ZX_OK;
    }

    void SetDriverShutdownComplete() {
      ZX_ASSERT(driver_shutting_down_);
      // We should have already called the shutdown observer.
      ZX_ASSERT(!shutdown_callback_);
      driver_shutting_down_ = false;
    }

    // Returns whether all dispatchers owned by the driver have completed shutdown.
    bool CompletedShutdown() { return dispatchers_.is_empty(); }

    // Returns whether the driver is currently being shut down.
    bool IsShuttingDown() { return driver_shutting_down_; }

    // Returns whether there are dispatchers that have not yet been removed with |RemoveDispatcher|.
    bool HasDispatchers() { return !dispatchers_.is_empty() || !shutdown_dispatchers_.is_empty(); }

    void ObserverCallStarted() { num_pending_observer_calls_++; }

    void ObserverCallComplete() {
      ZX_ASSERT(num_pending_observer_calls_ > 0);
      num_pending_observer_calls_--;
    }

    DriverShutdownCallback take_driver_shutdown_callback() {
      auto callback = std::move(shutdown_callback_);
      shutdown_callback_ = nullptr;
      return callback;
    }

    fbl::RefPtr<driver_runtime::Dispatcher> initial_dispatcher() { return initial_dispatcher_; }

    uint32_t num_pending_observer_calls() const { return num_pending_observer_calls_; }

   private:
    const void* driver_ = nullptr;
    // Dispatchers that have been shutdown.
    fbl::DoublyLinkedList<fbl::RefPtr<driver_runtime::Dispatcher>> shutdown_dispatchers_;
    // All other dispatchers owned by |driver|.
    fbl::DoublyLinkedList<fbl::RefPtr<driver_runtime::Dispatcher>> dispatchers_;
    // The first dispatcher created for the driver.
    fbl::RefPtr<driver_runtime::Dispatcher> initial_dispatcher_ = nullptr;
    // Whether the driver is in the process of shutting down.
    bool driver_shutting_down_ = false;
    // The callback which will be invoked once shutdown completes.
    DriverShutdownCallback shutdown_callback_ = nullptr;
    // The number of threads currently calling a dispatcher shutdown observer handler
    // for a dispatcher.
    uint32_t num_pending_observer_calls_ = 0;

    fdf_env_resume_requester_t* resume_requester_ = nullptr;
  };

  // Make sure this destructs after |loop_|. This is as dispatchers will remove themselves
  // from this list on shutdown.
  fbl::Mutex lock_;
  // Maps from driver owner to driver state.
  fbl::WAVLTree<const void*, fbl::RefPtr<DriverState>> drivers_ __TA_GUARDED(&lock_);
  // Notified when all drivers are destroyed.
  fbl::ConditionVariable drivers_destroyed_event_ __TA_GUARDED(&lock_);

  // Thread pools which have scheduler roles.
  fbl::WAVLTree<std::string, std::unique_ptr<ThreadPool>> role_to_thread_pool_ __TA_GUARDED(&lock_);
  // Thread pool which has no scheduler role applied.
  // This must come after |role_thread_pools_|, so that we shutdown the loop first,
  // in case we have any scheduled tasks to delete thread pools.
  ThreadPool default_thread_pool_;
  // Thread pool that is not managed.
  std::optional<ThreadPool> unmanaged_thread_pool_;
  zx::result<ThreadPool*> GetOrCreateThreadPoolLocked(std::string_view scheduler_role)
      __TA_REQUIRES(&lock_);

  zx_status_t RegisterDispatcherLocked(fbl::RefPtr<Dispatcher> dispatcher, ThreadPool* thread_pool,
                                       std::unique_ptr<EventWaiter> event_waiter)
      __TA_REQUIRES(&lock_);

  // The options that were passed to the last call to |Start|.
  // Static to make lookup fast in hot paths.
  static uint32_t options_;

  // Number of threads that are in the process of handling |NotifyDispatcherShutdown| events.
  uint32_t num_notify_shutdown_threads_ = 0;
  TokenManager token_manager_;

  std::atomic<fdf_env_stall_scanner_t*> stall_scanner_ = nullptr;
};

// Returns the currently active dispatcher coordinator
DispatcherCoordinator& GetDispatcherCoordinator();

}  // namespace driver_runtime

#endif  // SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_COORDINATOR_H_
