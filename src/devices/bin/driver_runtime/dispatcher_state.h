// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_STATE_H_
#define SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_STATE_H_

#include <lib/async/dispatcher.h>
#include <lib/async/task.h>

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>

#include <fbl/string.h>

namespace driver_runtime {

class Dispatcher;

enum class DispatcherState {
  // The dispatcher is running and accepting new requests.
  kRunning,
  // The dispatcher is in the process of shutting down.
  kShuttingDown,
  // The dispatcher has completed shutdown and can be destroyed.
  kShutdown,
  // The dispatcher is about to be destroyed.
  kDestroyed,
};

// Why a request was not inlined.
enum NonInlinedReason : uint8_t {
  // Dispatcher has the ALLOW_SYNC_CALLS option set.
  kAllowSyncCalls,
  // The dispatcher is already handling a request on another thread.
  kDispatchingOnAnotherThread,
  // It was a posted task.
  kTask,
  // We are queueing to a dispatcher that is running on a non-runtime managed thread.
  kUnknownThread,
  // We are queueing to a dispatcher that is already in the callstack.
  kReentrant,
  // The channel received a message, but no channel read was registered yet.
  kChannelWaitNotYetRegistered,
  // We are queueing to a dispatcher that does not allow thread migration
  kNoThreadMigration,
};

struct DebugStats {
  // Counts the number of occurrences of each reason for why a request was not-inlined.
  struct NonInlinedStats {
    size_t allow_sync_calls = 0;
    size_t parallel_dispatch = 0;
    size_t task = 0;
    size_t unknown_thread = 0;
    size_t reentrant = 0;
    size_t channel_wait_not_yet_registered = 0;
    size_t no_thread_migration = 0;
  };

  NonInlinedStats non_inlined = {};

  size_t num_inlined_requests = 0;
  size_t num_total_requests = 0;
};

struct TaskDebugInfo {
  async_task_t* ptr;
  async_task_handler_t* handler;
  Dispatcher* initiating_dispatcher;
  const void* initiating_driver;
};

// Holds debug information for the current dispatcher state.
// Pointers are not guaranteed to stay valid and are for identification purposes only.
struct DumpState {
  // The dispatcher that is running on the current thread.
  // Will be NULL if the thread is not managed by the driver runtime.
  Dispatcher* running_dispatcher;
  const void* running_driver;
  // The dispatcher that has been requested to be dumped to the log.
  Dispatcher* dispatcher_to_dump;
  // State of |dispatcher_to_dump|.
  const void* driver_owner;
  fbl::String name;
  bool synchronized;
  bool allow_sync_calls;
  DispatcherState state;
  std::vector<TaskDebugInfo> queued_tasks;
  DebugStats debug_stats;
  // If a call to |Destroy| has been made, this will store the name of the dispatcher that made
  // the call. This is useful if multiple calls to |Destroy| are erroneously made and there is
  // still a ptr to the dispatcher keeping it alive.
  std::string dispatcher_destroy_context;
  // If true, |Destroy| was called by the user via |fdf_dispatcher_destroy|,
  // otherwise |Destroy| was called by the environment via |fdf_env_destroy_all_dispatchers|.
  std::optional<bool> dispatcher_destroy_user_initiated;
};

}  // namespace driver_runtime

#endif  // SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_STATE_H_
