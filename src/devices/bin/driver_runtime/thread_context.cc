// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_runtime/thread_context.h"

#include <lib/fit/defer.h>
#include <lib/sync/cpp/mutex.h>
#include <lib/zx/thread.h>
#include <zircon/assert.h>

#include <mutex>
#include <vector>

#include "src/devices/bin/driver_runtime/dispatcher.h"
#include "src/devices/lib/log/log.h"

namespace {

struct Entry {
  const void* driver;
  driver_runtime::Dispatcher* dispatcher;
};

class CurrentDriverTracker {
 public:
  void Add(zx_koid_t koid, const void** current_driver_holder) {
    std::lock_guard guard(mutex_);
    current_driver_holder_for_tid_[koid] = current_driver_holder;
  }
  void Remove(zx_koid_t koid) {
    std::lock_guard guard(mutex_);
    current_driver_holder_for_tid_.erase(koid);
  }

  std::optional<const void*> TryGet(zx_koid_t tid) {
    std::lock_guard guard(mutex_);
    auto entry = current_driver_holder_for_tid_.find(tid);
    if (entry == current_driver_holder_for_tid_.end()) {
      return std::nullopt;
    }
    const void** current_driver_holder = entry->second;
    if (!current_driver_holder || !*current_driver_holder) {
      return std::nullopt;
    }

    return *current_driver_holder;
  }

 private:
  libsync::Mutex mutex_;
  std::unordered_map<zx_koid_t, const void**> current_driver_holder_for_tid_ __TA_GUARDED(mutex_);
};

// Caching the koid for our thread in a thread local, so that we don't have to call the get_info
// every time we want to ensure we have an entry in the global current driver map.
thread_local zx_koid_t g_thread_koid = []() {
  zx_info_handle_basic_t basic_info;
  zx_status_t status = zx::thread::self()->get_info(ZX_INFO_HANDLE_BASIC, &basic_info,
                                                    sizeof(basic_info), nullptr, nullptr);
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to get thread info.");
  return basic_info.koid;
}();

static thread_local std::vector<Entry> g_driver_call_stack;
static thread_local const void* g_current_thread_driver;

static thread_local Entry g_default_testing_state = {nullptr, nullptr};
// The latest generation seen by this thread.
static thread_local uint32_t g_cached_irqs_generation = 0;
// The result of setting the role profile for the current thread.
// May be std::nullopt if no attempt has been made to set the role profile.
static thread_local std::optional<zx_status_t> g_role_profile_status;
// A shared_ptr to an atomic time value for when this thread last entered a callback, or 0 if not
// currently running a task.
thread_local std::atomic_int64_t g_task_entry_time = -1;

// This global will be used to maintain a mapping to the atomic holding the active driver for each
// thread. This is then used to service the GetDriverOnTid requests to locate the thread local
// driver that is running.
CurrentDriverTracker g_current_driver_for_tid = {};

// Use a thread local bool to track this so we don't have to go into the tracker which requires
// locking a mutex.
thread_local bool g_added_to_tracker = false;

// Remove the current driver entry when the thread is destroyed.
thread_local fit::deferred_callback g_remove_current_driver = fit::defer_callback([]() {
  g_current_driver_for_tid.Remove(g_thread_koid);
  g_added_to_tracker = false;
});

}  // namespace

namespace thread_context {

void PushDriver(const void* driver, driver_runtime::Dispatcher* dispatcher) {
  // TODO(https://fxbug.dev/42169761): re-enable this once driver host v1 is deprecated.
  // ZX_DEBUG_ASSERT(IsDriverInCallStack(driver) == false);
  if (IsDriverInCallStack(driver)) {
    LOGF(TRACE, "ThreadContext: tried to push driver %p that was already in stack\n", driver);
  }
  g_driver_call_stack.push_back({driver, dispatcher});
  g_current_thread_driver = driver;

  if (unlikely(!g_added_to_tracker)) {
    // Activates the g_current_driver_for_tid entry as the current thread now has an active driver
    g_current_driver_for_tid.Add(g_thread_koid, &g_current_thread_driver);
    g_added_to_tracker = true;
  }
}

void PopDriver() {
  ZX_ASSERT(!g_driver_call_stack.empty());
  g_driver_call_stack.pop_back();

  // Assign the previous driver in the stack as the current driver.
  if (!g_driver_call_stack.empty()) {
    g_current_thread_driver = g_driver_call_stack.back().driver;
  } else {
    g_current_thread_driver = nullptr;
  }
}

const void* GetCurrentDriver() {
  return g_driver_call_stack.empty() ? g_default_testing_state.driver
                                     : g_driver_call_stack.back().driver;
}

driver_runtime::Dispatcher* GetCurrentDispatcher() {
  return g_driver_call_stack.empty() ? g_default_testing_state.dispatcher
                                     : g_driver_call_stack.back().dispatcher;
}

void SetDefaultTestingDispatcher(driver_runtime::Dispatcher* dispatcher) {
  g_default_testing_state.dispatcher = dispatcher;
  g_default_testing_state.driver = dispatcher ? dispatcher->owner() : nullptr;
}

bool IsDriverInCallStack(const void* driver) {
  for (int64_t i = g_driver_call_stack.size() - 1; i >= 0; i--) {
    if (g_driver_call_stack[i].driver == driver) {
      return true;
    }
  }
  return false;
}

bool IsCallStackEmpty() { return g_driver_call_stack.empty(); }

uint32_t GetIrqGenerationId() { return g_cached_irqs_generation; }

void SetIrqGenerationId(uint32_t id) { g_cached_irqs_generation = id; }

std::optional<zx_status_t> GetRoleProfileStatus() { return g_role_profile_status; }

void SetRoleProfileStatus(zx_status_t status) { g_role_profile_status = status; }

zx::result<const void*> GetDriverOnTid(zx_koid_t tid) {
  auto entry = g_current_driver_for_tid.TryGet(tid);
  if (entry) {
    return zx::ok(entry.value());
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

std::pair<zx_koid_t, std::atomic_int64_t*> GetTaskEntryTimeSlot() {
  return std::make_pair(g_thread_koid, &g_task_entry_time);
}

}  // namespace thread_context
