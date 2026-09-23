// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thread_pool.h"

#include <fidl/fuchsia.scheduler/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/thread.h>

#include "dispatcher_coordinator.h"
#include "dispatcher_internals.h"
#include "src/devices/lib/log/log.h"

namespace driver_runtime {

void ThreadPool::ThreadWakeupPrologue() {
  zx_instant_mono_t entry_time = zx_clock_get_monotonic();
  std::pair<zx_koid_t, std::atomic_int64_t*> task_entry_slot =
      thread_context::GetTaskEntryTimeSlot();
  if (unlikely(*task_entry_slot.second == -1)) {
    // Store the pointer to our thread's entry time slot in the thread pool's list so that it can be
    // checked for stalled threads by the environment.
    //
    // Thread safety note: While the pointer stored in `thread_entry_time_slots_` may outlive the
    // runtime of the thread, and so the TLS variable it's stored in, the dispatcher will remove the
    // thread pool from the list consulted to check for stalled tasks before scheduling the
    // destruction of the pool, so it should not be accessed after the thread has stopped.
    fbl::AutoLock guard(&lock_);
    thread_entry_time_slots_.push_back(task_entry_slot);
  }
  task_entry_slot.second->store(entry_time);
  if (++threads_entered_ == num_threads()) {
    driver_runtime::GetDispatcherCoordinator().TriggerStallScanner();
  }
  if (scheduler_role_ != kNoSchedulerRole) {
    if (thread_context::GetRoleProfileStatus().has_value()) {
      // We have already attempted to set the role profile for the current thread.
      return;
    }
    zx_status_t status = SetRoleProfile();
    if (status != ZX_OK) {
      // Failing to set the role profile is not a fatal error.
      LOGF(WARNING, "Failed to set scheduler role: %d", status);
    }
    thread_context::SetRoleProfileStatus(status);
  }
}

void ThreadPool::ThreadWakeupEpilogue() {
  thread_context::GetTaskEntryTimeSlot().second->store(0);
  --threads_entered_;
}

bool ThreadPool::ScanThreadsForStalls() {
  fbl::AutoLock lock(&lock_);
  zx::time current_time = zx::time(zx_clock_get_monotonic());
  zx::time stalled_time = current_time - zx::msec(kStallTimeMs);
  zx::time stale_time = current_time - zx::msec(kStaleTimeMs);
  uint32_t stalled_threads = 0;
  // TODO(468352723): Make these logs less annoying so they can be raised above DEBUG
  for (auto& slot : thread_entry_time_slots_) {
    zx::time timestamp(slot.second->load());
    if (timestamp != zx::time(0) && timestamp < stalled_time) {
      if (timestamp > stale_time) {
        LOGF(DEBUG, "Found a thread (id: %u, role: '%s') that has been stalled for %ld ms",
             slot.first, scheduler_role_.c_str(), (current_time - timestamp).to_msecs());
      }
      stalled_threads++;
    }
  }
  if (num_threads_ > 0 && num_threads_ < MaxThreadsLocked() && stalled_threads >= num_threads_) {
    // if we weren't already stalled try to spawn a new thread.
    if (!stalled_) {
      LOGF(
          DEBUG,
          "All threads on thread pool (role: '%s') are stalled (%d/%d). Spawning a new thread, if possible (max threads: %d).",
          scheduler_role_.c_str(), stalled_threads, num_threads_, MaxThreadsLocked());
      stalled_ = true;
      AddThreadLocked();
    }
    return false;
  } else {
    // clear the stalled flag if it was set so we can warn if we become stuck again.
    stalled_ = false;
    return num_threads_ == threads_entered_.load();
  }
}

zx_status_t ThreadPool::SetRoleProfile() {
#if FUCHSIA_API_LEVEL_AT_LEAST(20)
  zx::result client_end = component::Connect<fuchsia_scheduler::RoleManager>();
  if (client_end.is_error()) {
    return client_end.status_value();
  }
  auto role_manager = *std::move(client_end);

  const zx_rights_t kRights = ZX_RIGHT_TRANSFER | ZX_RIGHT_MANAGE_THREAD;
  zx::thread duplicate;

  zx_status_t status = zx::thread::self()->duplicate(kRights, &duplicate);
  if (status != ZX_OK) {
    return status;
  }

  fidl::Arena arena;
  auto request =
      fuchsia_scheduler::wire::RoleManagerSetRoleRequest::Builder(arena)
          .target(fuchsia_scheduler::wire::RoleTarget::WithThread(std::move(duplicate)))
          .role(fuchsia_scheduler::wire::RoleName{fidl::StringView::FromExternal(scheduler_role())})
          .Build();
  auto result = fidl::WireCall(role_manager)->SetRole(request);
  if (result.status() != ZX_OK) {
    return result.status();
  }
  if (!result.value().is_ok()) {
    return result.value().error_value();
  }
  return ZX_OK;
#endif
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t ThreadPool::AddThread() {
  if (is_unmanaged_) {
    // No-op for the unmanaged thread-pool.
    return ZX_OK;
  }

  fbl::AutoLock lock(&lock_);
  return AddThreadLocked();
}

zx_status_t ThreadPool::AddThreadLocked() {
  if (is_unmanaged_) {
    // No-op for the unmanaged thread-pool.
    return ZX_OK;
  }

  auto name = "fdf-dispatcher-thread-" + std::to_string(num_threads_);
  if (scheduler_role() != kNoSchedulerRole) {
    name += ":";
    name += scheduler_role();
  }
  zx_status_t status = loop_.StartThread(name.c_str());
  if (status == ZX_OK) {
    num_threads_++;
  }
  return status;
}

zx_status_t ThreadPool::OnDispatcherSealed() {
  fbl::AutoLock lock(&lock_);
  ZX_ASSERT(allow_sync_call_dispatchers_ > 0);
  allow_sync_call_dispatchers_--;
  return ZX_OK;
}

zx_status_t ThreadPool::OnDispatcherAdded(Dispatcher& dispatcher) {
  fbl::AutoLock lock(&lock_);
  if (dispatcher.allow_sync_calls()) {
    if ((allow_sync_call_dispatchers_ + 1) > thread_limit_) {
      LOGF(WARNING,
           "Dispatcher that allows sync calls created when already at thread pool thread limit");
      return ZX_ERR_NO_RESOURCES;
    }
    ++allow_sync_call_dispatchers_;
  }

  ++num_dispatchers_;

  if (DispatcherCoordinator::dynamic_thread_spawning()) {
    // only start a new thread if we're not dynamically managing threads
    return ZX_OK;
  }

  // We only want to spawn a thread if we don't have more threads than we should need to service
  // the types of dispatchers we have. See |MaxThreadsLocked| for the criteria we use to decide
  // on the maximum thread count.
  if (num_threads_ >= MaxThreadsLocked()) [[likely]] {
    LOGF(
        DEBUG,
        "Not spawning thread on thread pool (role: %s) because it would add more threads (%d) than dispatchers (%d) or max threads (%d)",
        scheduler_role_.c_str(), num_threads_, num_dispatchers_, MaxThreadsLocked());
    return ZX_OK;
  }
  return AddThreadLocked();
}

void ThreadPool::OnDispatcherRemoved(Dispatcher& dispatcher) {
  fbl::AutoLock lock(&lock_);

  if (dispatcher.allow_sync_calls()) {
    ZX_ASSERT(allow_sync_call_dispatchers_ > 0);
    allow_sync_call_dispatchers_--;
  }

  ZX_ASSERT(num_dispatchers_ > 0);
  num_dispatchers_--;
}

void ThreadPool::Reset() {
  {
    fbl::AutoLock lock(&lock_);
    ZX_ASSERT_MSG(allow_sync_call_dispatchers_ == 0,
                  "Resetting thread pool with sync dispatchers still active: %d",
                  allow_sync_call_dispatchers_);
  }

  loop_.Quit();
  loop_.JoinThreads();
  loop_.ResetQuit();
  loop_.RunUntilIdle();

  {
    fbl::AutoLock al(&lock_);
    thread_limit_ = kDefaultThreadLimit;
    num_threads_ = 0;
    allow_sync_call_dispatchers_ = 0;
    num_dispatchers_ = 0;
  }
}

void ThreadPool::CacheUnboundIrq(std::unique_ptr<AsyncIrq> irq) {
  fbl::AutoLock lock(&lock_);
  cached_irqs_.AddIrqLocked(std::move(irq));
}

void ThreadPool::OnThreadWakeup() {
  uint32_t thread_irq_generation_id = thread_context::GetIrqGenerationId();
  // Check if we have already tracked this thread wakeup for the current generation of irqs.
  // |cur_generatiom_id| is atomic - we do not acquire the lock here to avoid unnecessary lock
  // contention per thread wakeup. If the generation id changes in the meanwhile, the next wakeuup
  // of this thread can handle that.
  if (thread_irq_generation_id == cached_irqs_.cur_generation_id()) {
    return;
  }

  fbl::AutoLock lock(&lock_);
  // We should set this first, as |cached_irqs_.NewThreadWakeupLocked| may increment the generation
  // id if it clears the current generation.
  thread_context::SetIrqGenerationId(cached_irqs_.cur_generation_id());
  cached_irqs_.NewThreadWakeupLocked(num_threads_);
}

void ThreadPool::CachedIrqs::AddIrqLocked(std::unique_ptr<AsyncIrq> irq) {
  // Check if we are tracking a new generation of irqs.
  if (cur_generation_.is_empty()) {
    IncrementGenerationId();
  }
  // We should only add to the current generation of cached irqs if no thread has woken up yet.
  if (threads_wakeup_count_ == 0) {
    cur_generation_.push_back(std::move(irq));
  } else {
    next_generation_.push_back(std::move(irq));
  }
}

void ThreadPool::CachedIrqs::NewThreadWakeupLocked(uint32_t total_number_threads) {
  threads_wakeup_count_++;
  // If all threads have woken up since the current generation of cached irqs was populated,
  // we can be sure that no threads have a pending irq packet that correspond to these unbound irqs.
  if (threads_wakeup_count_ < total_number_threads) {
    return;
  }
  // Drop the current generation of irqs, and begin tracking thread wakeups for the next generation.
  cur_generation_ = std::move(next_generation_);
  // If the next generation already has irqs, we need to increment the generation counter
  // so that thread wakeups will be tracked.
  if (cur_generation_.size() > 0) {
    IncrementGenerationId();
  }
  threads_wakeup_count_ = 0;
}

}  // namespace driver_runtime
