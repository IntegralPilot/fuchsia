// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dispatcher_coordinator.h"

#include <lib/async/cpp/task.h>
#include <lib/fit/defer.h>

#include "dispatcher_internals.h"
#include "src/devices/lib/log/log.h"

namespace driver_runtime {

DispatcherCoordinator& GetDispatcherCoordinator() {
  static DispatcherCoordinator shared_loop;
  return shared_loop;
}

uint32_t DispatcherCoordinator::options_ = 0;
// static
void DispatcherCoordinator::WaitUntilDispatchersIdle() {
  std::vector<fbl::RefPtr<Dispatcher>> dispatchers;
  {
    fbl::AutoLock lock(&(GetDispatcherCoordinator().lock_));
    for (auto& driver : GetDispatcherCoordinator().drivers_) {
      driver.GetDispatchers(dispatchers);
    }
  }
  for (auto& d : dispatchers) {
    d->WaitUntilIdle();
  }
}

// static
void DispatcherCoordinator::WaitUntilDispatchersDestroyed() {
  auto& coordinator = GetDispatcherCoordinator();
  fbl::AutoLock lock(&coordinator.lock_);
  if (coordinator.AreAllDriversDestroyedLocked()) {
    return;
  }
  coordinator.drivers_destroyed_event_.Wait(&coordinator.lock_);
}

// static
zx_status_t DispatcherCoordinator::TestingRun(zx::time deadline, bool once) {
  std::optional<ThreadPool>& unmanaged_thread_pool =
      GetDispatcherCoordinator().unmanaged_thread_pool_;
  if (unmanaged_thread_pool.has_value()) {
    return unmanaged_thread_pool.value().loop()->Run(deadline, once);
  }

  return ZX_ERR_BAD_STATE;
}

// static
zx_status_t DispatcherCoordinator::TestingRunUntilIdle() {
  std::optional<ThreadPool>& unmanaged_thread_pool =
      GetDispatcherCoordinator().unmanaged_thread_pool_;
  if (unmanaged_thread_pool.has_value()) {
    return unmanaged_thread_pool.value().loop()->RunUntilIdle();
  }

  return ZX_ERR_BAD_STATE;
}

// static
void DispatcherCoordinator::TestingQuit() {
  std::optional<ThreadPool>& unmanaged_thread_pool =
      GetDispatcherCoordinator().unmanaged_thread_pool_;
  if (unmanaged_thread_pool.has_value()) {
    unmanaged_thread_pool.value().loop()->Quit();
  }
}

// static
zx_status_t DispatcherCoordinator::TestingResetQuit() {
  std::optional<ThreadPool>& unmanaged_thread_pool =
      GetDispatcherCoordinator().unmanaged_thread_pool_;
  if (unmanaged_thread_pool.has_value()) {
    return unmanaged_thread_pool.value().loop()->ResetQuit();
  }

  return ZX_ERR_BAD_STATE;
}

// static
zx_status_t DispatcherCoordinator::ShutdownDispatchersAsync(
    const void* driver, fdf_env_driver_shutdown_observer_t* observer) {
  DriverState::DriverShutdownCallback shutdown_callback;

  std::vector<fbl::RefPtr<Dispatcher>> dispatchers;
  {
    fbl::AutoLock lock(&(GetDispatcherCoordinator().lock_));
    auto driver_state_iter = GetDispatcherCoordinator().drivers_.find(driver);
    if (!driver_state_iter.IsValid()) {
      return ZX_ERR_INVALID_ARGS;
    }
    driver_state_iter->GetDispatchers(dispatchers);

    auto driver_state_ref = fbl::RefPtr<DriverState>(&(*driver_state_iter));
    shutdown_callback = [driver, observer, driver_state_ref = std::move(driver_state_ref)]() {
      observer->handler(driver, observer);
      driver_state_ref->SetDriverShutdownComplete();
    };
    // Set the driver state so that attempts to create new dispatchers on the driver
    // return an error.
    // If there are no dispatchers to shutdown, we will post a task to call the callback
    // immediately rather than set it in the driver state.
    auto status = driver_state_iter->SetDriverShuttingDown(
        dispatchers.empty() ? nullptr : std::move(shutdown_callback));
    if (status != ZX_OK) {
      return status;
    }
  }
  for (auto& dispatcher : dispatchers) {
    async::PostTask(dispatcher->GetAsyncDispatcher(), [=]() { dispatcher->ShutdownAsync(); });
  }
  if (dispatchers.empty()) {
    auto thread_pool = GetDispatcherCoordinator().default_thread_pool();
    ZX_ASSERT(shutdown_callback);
    // The dispatchers have already been shutdown and no calls to |NotifyDispatcherShutdown|
    // will occur, so we need to schedule the handler to be called.
    async::PostTask(thread_pool->loop()->dispatcher(),
                    [callback = std::move(shutdown_callback)]() mutable { callback(); });
  }
  return ZX_OK;
}

// static
zx_status_t DispatcherCoordinator::SuspendDispatchersAsync(const void* driver,
                                                           fdf_env_suspend_completer_t* completer) {
  if (!driver || !completer || !completer->handler) {
    return ZX_ERR_INVALID_ARGS;
  }

  std::vector<fbl::RefPtr<Dispatcher>> dispatchers;
  {
    fbl::AutoLock lock(&(GetDispatcherCoordinator().lock_));
    auto driver_state_iter = GetDispatcherCoordinator().drivers_.find(driver);
    if (!driver_state_iter.IsValid()) {
      return ZX_ERR_INVALID_ARGS;
    }
    driver_state_iter->GetDispatchers(dispatchers);
  }

  if (dispatchers.empty()) {
    completer->handler(completer);
    return ZX_OK;
  }

  struct SuspendContext {
    fdf_env_suspend_completer_t* completer;
    std::atomic<size_t> pending_dispatchers;
  };

  auto context = std::make_shared<SuspendContext>();
  context->completer = completer;
  context->pending_dispatchers.store(dispatchers.size());

  for (auto& dispatcher : dispatchers) {
    dispatcher->SuspendAsync([context]() {
      if (context->pending_dispatchers.fetch_sub(1) == 1) {
        context->completer->handler(context->completer);
      }
    });
  }
  return ZX_OK;
}

// static
void DispatcherCoordinator::ResumeDispatchers(const void* driver) {
  if (!driver) {
    return;
  }

  std::vector<fbl::RefPtr<Dispatcher>> dispatchers;
  {
    fbl::AutoLock lock(&(GetDispatcherCoordinator().lock_));
    auto driver_state_iter = GetDispatcherCoordinator().drivers_.find(driver);
    if (!driver_state_iter.IsValid()) {
      return;
    }
    driver_state_iter->GetDispatchers(dispatchers);
  }

  for (auto& dispatcher : dispatchers) {
    dispatcher->Resume();
  }
}

// static
void DispatcherCoordinator::RegisterResumeRequester(const void* driver,
                                                    fdf_env_resume_requester_t* requester) {
  std::vector<fbl::RefPtr<Dispatcher>> dispatchers;
  {
    fbl::AutoLock lock(&(GetDispatcherCoordinator().lock_));
    auto driver_state_iter = GetDispatcherCoordinator().drivers_.find(driver);
    if (!driver_state_iter.IsValid()) {
      return;
    }
    driver_state_iter->GetDispatchers(dispatchers);
    driver_state_iter->SetResumeRequester(requester);
  }

  for (auto& dispatcher : dispatchers) {
    dispatcher->SetResumeRequester(requester);
  }
}

// static
void DispatcherCoordinator::DestroyAllDispatchers() {
  std::vector<fbl::RefPtr<Dispatcher>> dispatchers;
  {
    fbl::AutoLock lock(&(GetDispatcherCoordinator().lock_));

    for (auto& driver_state : GetDispatcherCoordinator().drivers_) {
      // We should have already shutdown all dispatchers.
      ZX_ASSERT(driver_state.CompletedShutdown());
      ZX_ASSERT_MSG(
          driver_state.num_pending_observer_calls() == 0,
          "Attempted to destroy a dispatcher which was still in a shutdown observer callback");
      driver_state.GetShutdownDispatchers(dispatchers);
    }
  }

  for (auto& dispatcher : dispatchers) {
    dispatcher->Destroy(false /* user_initiated */);
  }

  WaitUntilDispatchersDestroyed();
}

// static
zx_status_t DispatcherCoordinator::TokenRegister(zx_handle_t token, fdf_dispatcher_t* dispatcher,
                                                 fdf_token_t* handler) {
  DispatcherCoordinator& coordinator = GetDispatcherCoordinator();
  return coordinator.token_manager_.Register(token, dispatcher, handler);
}

// static
zx_status_t DispatcherCoordinator::TokenReceive(zx_handle_t token, fdf_handle_t* handle) {
  DispatcherCoordinator& coordinator = GetDispatcherCoordinator();
  return coordinator.token_manager_.Receive(token, handle);
}

// static
zx_status_t DispatcherCoordinator::TokenTransfer(zx_handle_t token, fdf_handle_t handle) {
  DispatcherCoordinator& coordinator = GetDispatcherCoordinator();
  return coordinator.token_manager_.Transfer(token, handle);
}

zx_status_t DispatcherCoordinator::AddDispatcher(fbl::RefPtr<Dispatcher> dispatcher,
                                                 std::string_view scheduler_role,
                                                 std::unique_ptr<EventWaiter> event_waiter) {
  fbl::AutoLock lock(&lock_);

  ThreadPool* thread_pool = default_thread_pool();
  if (scheduler_role != ThreadPool::kNoSchedulerRole) {
    auto result = GetOrCreateThreadPoolLocked(scheduler_role);
    if (result.is_error()) {
      return result.status_value();
    }
    thread_pool = *result;
  }

  bool thread_pool_no_sync_calls =
      thread_pool->scheduler_role_options() & FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS;
  // don't allow a sync calls dispatcher that is also unsynchronized or running on a no sync calls
  // thread pool.
  if (dispatcher->allow_sync_calls() &&
      (dispatcher->unsynchronized() || thread_pool_no_sync_calls)) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  bool no_thread_migration = dispatcher->options() & FDF_DISPATCHER_OPTION_NO_THREAD_MIGRATION;
  // only allow limiting thread migration when on a no sync calls dispatcher *and* scheduler role
  if (no_thread_migration && (!thread_pool_no_sync_calls || dispatcher->allow_sync_calls())) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  uint32_t dispatchers_before = thread_pool->num_dispatchers();

  // This may fail if the entire driver is being shut down by the driver host.
  zx_status_t status = RegisterDispatcherLocked(dispatcher, thread_pool, std::move(event_waiter));
  if (status != ZX_OK) {
    return status;
  }

  if (scheduler_role != ThreadPool::kNoSchedulerRole && dispatchers_before == 0) {
    status = async::PostTask(thread_pool->loop()->dispatcher(), [dispatcher]() mutable {
      // Each thread in the thread pool will check whether it needs to set the scheduler
      // role when it wakes up. If this is the first dispatcher, we might as well
      // post a task so we can get the scheduler role set ASAP. Otherwise, if there
      // are multiple dispatchers and threads, it becomes less likely that we would
      // happen to post the task to a thread that doesn't already have the scheduler role set,
      // so we'll leave any unset thread to set it's role on next wakeup.
    });
    if (status != ZX_OK) {
      LOGF(ERROR, "Failed to post task to set scheduler role");
    }
  }

  return ZX_OK;
}

zx_status_t DispatcherCoordinator::AddUnmanagedDispatcher(
    fbl::RefPtr<Dispatcher> dispatcher, std::unique_ptr<EventWaiter> event_waiter) {
  fbl::AutoLock lock(&lock_);

  auto* thread_pool = GetOrCreateUnmanagedThreadPool();
  return RegisterDispatcherLocked(dispatcher, thread_pool, std::move(event_waiter));
}

zx_status_t DispatcherCoordinator::RegisterDispatcherLocked(
    fbl::RefPtr<Dispatcher> dispatcher, ThreadPool* thread_pool,
    std::unique_ptr<EventWaiter> event_waiter) {
  auto driver_state = drivers_.find(dispatcher->owner());
  if (driver_state == drivers_.end()) {
    auto new_driver_state = fbl::AdoptRef(new DriverState(dispatcher->owner()));
    drivers_.insert(new_driver_state);
    driver_state = drivers_.find(dispatcher->owner());
  } else {
    // If the driver is shutting down, we should not allow creating new dispatchers.
    if (driver_state->IsShuttingDown()) {
      return ZX_ERR_BAD_STATE;
    }
  }

  zx_status_t status = thread_pool->OnDispatcherAdded(*dispatcher);
  if (status != ZX_OK) {
    return status;
  }

  dispatcher->SetEventWaiter(event_waiter.get());
  status = EventWaiter::BeginWaitWithRef(std::move(event_waiter), dispatcher,
                                         thread_pool->loop()->dispatcher());
  if (status != ZX_OK) {
    thread_pool->OnDispatcherRemoved(*dispatcher);
    if (thread_pool->num_dispatchers() == 0) {
      DestroyThreadPool(thread_pool);
    }
    dispatcher->SetEventWaiter(nullptr);
    return status;
  }

  dispatcher->SetThreadPool(thread_pool, thread_pool->loop()->dispatcher());
  dispatcher->SetResumeRequester(driver_state->GetResumeRequester());
  driver_state->AddDispatcher(std::move(dispatcher));

  return ZX_OK;
}

// static
uint32_t DispatcherCoordinator::GetThreadLimit(std::string_view scheduler_role) {
  auto& coordinator = GetDispatcherCoordinator();
  fbl::AutoLock lock(&coordinator.lock_);

  auto thread_pool = coordinator.default_thread_pool();
  if (scheduler_role != ThreadPool::kNoSchedulerRole) {
    auto iter = coordinator.role_to_thread_pool_.find(std::string(scheduler_role));
    if (iter == coordinator.role_to_thread_pool_.end()) {
      return ThreadPool::kDefaultThreadLimit;
    }
    thread_pool = &(*iter);
  }
  return thread_pool->thread_limit();
}

// static
zx_status_t DispatcherCoordinator::SetThreadLimit(std::string_view scheduler_role,
                                                  uint32_t max_threads) {
  auto& coordinator = GetDispatcherCoordinator();
  fbl::AutoLock lock(&coordinator.lock_);

  auto thread_pool = coordinator.default_thread_pool();
  if (scheduler_role != ThreadPool::kNoSchedulerRole) {
    auto result = coordinator.GetOrCreateThreadPoolLocked(scheduler_role);
    if (result.is_error()) {
      return result.error_value();
    }
    thread_pool = *result;
  }
  return thread_pool->set_thread_limit(max_threads);
}

// static
uint32_t DispatcherCoordinator::GetSchedulerRoleOpts(std::string_view scheduler_role) {
  auto& coordinator = GetDispatcherCoordinator();
  fbl::AutoLock lock(&coordinator.lock_);

  auto thread_pool = coordinator.default_thread_pool();
  if (scheduler_role != ThreadPool::kNoSchedulerRole) {
    auto iter = coordinator.role_to_thread_pool_.find(std::string(scheduler_role));
    if (iter == coordinator.role_to_thread_pool_.end()) {
      return 0;
    }
    thread_pool = &(*iter);
  }
  return thread_pool->scheduler_role_options();
}

// static
zx_status_t DispatcherCoordinator::SetSchedulerRoleOpts(std::string_view scheduler_role,
                                                        uint32_t opts) {
  auto& coordinator = GetDispatcherCoordinator();
  fbl::AutoLock lock(&coordinator.lock_);

  auto thread_pool = coordinator.default_thread_pool();
  if (scheduler_role != ThreadPool::kNoSchedulerRole) {
    auto result = coordinator.GetOrCreateThreadPoolLocked(scheduler_role);
    if (result.is_error()) {
      return result.error_value();
    }
    thread_pool = *result;
  }
  return thread_pool->set_scheduler_role_options(opts);
}

zx_duration_mono_t DispatcherCoordinator::ScanThreadsForStalls() {
  fbl::AutoLock lock(&lock_);
  bool scan_again = default_thread_pool_.ScanThreadsForStalls();
  // Thread safety note: It is important that thread pools be removed from this list before
  // the threads in them stop so that this function won't try to access memory tied to the lifetime
  // of the thread. If ASAN triggers anywhere in here, that means this invariant has been broken.
  // See the comment in `ThreadWakeupPrologue` for more information.
  for (auto& thread_pool : role_to_thread_pool_) {
    scan_again = thread_pool.ScanThreadsForStalls() || scan_again;
  }
  // tell the caller to check again in half the stall time, so we worst-case to finding stalled
  // threads in (stalltime*1.5).
  if (!scan_again) {
    return 0;
  }
  return zx::msec(kStallTimeMs / 2).get();
}

void DispatcherCoordinator::RegisterStallScanner(fdf_env_stall_scanner_t* stall_scanner) {
  stall_scanner_.store(stall_scanner);
}

void DispatcherCoordinator::TriggerStallScanner() {
  auto* stall_scanner = stall_scanner_.load();
  if (stall_scanner != nullptr) {
    stall_scanner->handler(stall_scanner, zx::msec(kStallTimeMs / 2).get());
  }
}

void DispatcherCoordinator::NotifyDispatcherShutdown(
    Dispatcher& dispatcher, fdf_dispatcher_shutdown_observer_t* dispatcher_shutdown_observer) {
  DriverState::DriverShutdownCallback shutdown_callback = nullptr;
  fbl::RefPtr<Dispatcher> initial_dispatcher;
  fbl::RefPtr<DriverState> driver_state;

  auto dec = fit::defer([&]() {
    fbl::AutoLock lock(&lock_);
    num_notify_shutdown_threads_--;
    // The last dispatcher may have been destroyed during a shutdown handler, so check
    // if all drivers have been destroyed.
    if (AreAllDriversDestroyedLocked()) {
      drivers_destroyed_event_.Broadcast();
    }
  });

  {
    fbl::AutoLock lock(&lock_);
    num_notify_shutdown_threads_++;

    auto driver_state_iter = drivers_.find(dispatcher.owner());
    ZX_ASSERT(driver_state_iter != drivers_.end());
    driver_state = fbl::RefPtr<DriverState>(&(*driver_state_iter));
    // Prepare to call the dispatcher's shutdown observer.
    // We need to set the dispatcher as shutdown beforehand, in case the user tries to
    // destroy the dispatcher.
    driver_state->SetDispatcherShutdown(dispatcher);
    driver_state->ObserverCallStarted();
  }

  // We need to call the dispatcher shutdown observer before calling the driver shutdown observer
  // (if any).
  if (dispatcher_shutdown_observer) {
    // We should have already set up the driver call stack before calling
    // |NotifyDispatcherShutdown|.
    ZX_ASSERT(thread_context::GetCurrentDispatcher() == &dispatcher);
    dispatcher_shutdown_observer->handler(dispatcher.to_fdf_dispatcher(),
                                          dispatcher_shutdown_observer);
  }
  {
    fbl::AutoLock lock(&lock_);
    driver_state->ObserverCallComplete();
    // Check if we are still waiting for dispatchers to complete shutting down.
    if (!driver_state->CompletedShutdown()) {
      return;
    }
    // Check that we are the last shutdown handler, so we don't
    // call any driver shutdown observer before all dispatcher shutdown observers have returned.
    if (driver_state->num_pending_observer_calls() > 0) {
      return;
    }
    // We should take ownership of the driver shutdown callback before dropping the lock.
    // This ensures we do not attempt to call it multiple times.
    shutdown_callback = driver_state->take_driver_shutdown_callback();
    if (!shutdown_callback) {
      // No one to notify.
      return;
    }
    // There should always be an initial dispatcher, as the dispatcher is the one that calls
    // |NotifyDispatcherShutdown|.
    initial_dispatcher = driver_state->initial_dispatcher();
    ZX_ASSERT(initial_dispatcher != nullptr);
  }
  {
    // Make sure the shutdown context looks like it is happening from the initial
    // dispatcher's thread.
    thread_context::PushDriver(initial_dispatcher->owner(), initial_dispatcher.get());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    shutdown_callback();
  }
}

void DispatcherCoordinator::RemoveDispatcher(Dispatcher& dispatcher) {
  fbl::AutoLock lock(&lock_);

  auto driver_state = drivers_.find(dispatcher.owner());
  ZX_ASSERT(driver_state != drivers_.end());

  auto thread_pool = dispatcher.thread_pool();
  thread_pool->OnDispatcherRemoved(dispatcher);
  if (thread_pool->num_dispatchers() == 0) {
    DestroyThreadPool(thread_pool);
  }

  driver_state->RemoveDispatcher(dispatcher);
  // If all dispatchers have been destroyed, the driver can be removed from the
  // driver state map.
  if (!driver_state->HasDispatchers()) {
    drivers_.erase(driver_state);
  }

  if (AreAllDriversDestroyedLocked()) {
    drivers_destroyed_event_.Broadcast();
  }
}

zx_status_t DispatcherCoordinator::Start(uint32_t options) {
  DispatcherCoordinator& coordinator = GetDispatcherCoordinator();
  fbl::AutoLock lock(&coordinator.lock_);
  auto thread_pool = coordinator.default_thread_pool();
  if (thread_pool->num_threads() != 0) {
    return ZX_ERR_BAD_STATE;
  }
  options_ = options;
  // pre-start the first dispatcher thread
  return thread_pool->AddThread();
}

// static
void DispatcherCoordinator::EnvReset() {
  DispatcherCoordinator& coordinator = GetDispatcherCoordinator();
  coordinator.Reset();
}

void DispatcherCoordinator::Reset() {
  {
    fbl::AutoLock al(&lock_);
    ZX_ASSERT(drivers_.is_empty());
  }

  default_thread_pool()->Reset();
  if (unmanaged_thread_pool_.has_value()) {
    unmanaged_thread_pool_.value().Reset();
  }
  unmanaged_thread_pool_.reset();
  options_ = 0;
}

std::optional<ThreadPool*> DispatcherCoordinator::GetThreadPool(std::string_view scheduler_role) {
  fbl::AutoLock al(&lock_);
  auto iter = role_to_thread_pool_.find(std::string(scheduler_role));
  if (iter != role_to_thread_pool_.end()) {
    return std::make_optional(&(*iter));
  }
  return std::nullopt;
}

zx::result<ThreadPool*> DispatcherCoordinator::GetOrCreateThreadPool(
    std::string_view scheduler_role) {
  fbl::AutoLock al(&lock_);
  return GetOrCreateThreadPoolLocked(scheduler_role);
}

zx::result<ThreadPool*> DispatcherCoordinator::GetOrCreateThreadPoolLocked(
    std::string_view scheduler_role) {
  auto iter = role_to_thread_pool_.find(std::string(scheduler_role));
  if (iter != role_to_thread_pool_.end()) {
    return zx::ok(&(*iter));
  }

  if (!AllowedSchedulerRoles::Get()->IsAllowed(scheduler_role)) {
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }

  auto thread_pool = std::make_unique<ThreadPool>(scheduler_role);
  zx_status_t status = thread_pool->AddThread();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  auto* thread_pool_ptr = thread_pool.get();
  role_to_thread_pool_.insert(std::move(thread_pool));
  return zx::ok(thread_pool_ptr);
}

void DispatcherCoordinator::DestroyThreadPool(ThreadPool* thread_pool) {
  if (thread_pool == default_thread_pool()) {
    return;
  }

  if (unmanaged_thread_pool_.has_value() && thread_pool == &unmanaged_thread_pool_.value()) {
    return;
  }

  // We should immediately remove the thread pool from the coordinator
  // map, so that a new driver doesn't try to use a destructing thread pool.
  std::unique_ptr<ThreadPool> owned_thread_pool = role_to_thread_pool_.erase(*thread_pool);
  ZX_ASSERT(owned_thread_pool != nullptr);

  // Ensure we are running on a default thread pool thread.
  async::PostTask(default_thread_pool()->loop()->dispatcher(),
                  [thread_pool = std::move(owned_thread_pool)]() {
                    // This will destruct the thread pool and join with its threads.
                  });
}

}  // namespace driver_runtime
