// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/time.h>
#include <lib/async/task.h>
#include <lib/async_patterns/cpp/task_scope.h>
#include <zircon/assert.h>

#include <mutex>

namespace async_patterns {

namespace {

constexpr char kTaskQueueThreadSafetyDescription[] = "|async_patterns::TaskQueue| is thread-unsafe";

}  // namespace

TaskScope::TaskScope(async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher),
      checker_(dispatcher, kTaskQueueThreadSafetyDescription),
      queue_(dispatcher, kTaskQueueThreadSafetyDescription) {}

TaskScope::~TaskScope() {
  std::scoped_lock lock{checker_};
  stopped_ = true;
  queue_.Stop();

  list_node_t* node = nullptr;
  list_node_t* temp_node = nullptr;
  list_for_every_safe(&delayed_task_list_, node, temp_node) {
    auto* task = static_cast<DelayedTask*>(node);
    zx_status_t status = async_cancel_task(dispatcher_, task);
    ZX_DEBUG_ASSERT(status == ZX_OK);
    RemoveDelayedTaskLocked(task);
  }

  ZX_ASSERT(list_is_empty(&delayed_task_list_));
}

TaskScope::DelayedTask::DelayedTask(TaskScope* owner)
    : list_node_t(LIST_INITIAL_CLEARED_VALUE),
      async_task_t{{ASYNC_STATE_INIT}, &DelayedTask::Handler, ZX_TIME_INFINITE_PAST},
      owner_(owner) {}

bool TaskScope::DelayedTask::Post(async_dispatcher_t* dispatcher, zx::time deadline) {
  async_task_t::deadline = deadline.get();
  return async_post_task(dispatcher, this) == ZX_OK;
}

void TaskScope::DelayedTask::Handler(async_dispatcher_t* dispatcher, async_task_t* task,
                                     zx_status_t status) {
  auto* self = static_cast<DelayedTask*>(task);
  if (status == ZX_OK) {
    self->Run();
  }
  self->owner_->RemoveDelayedTask(self);
}

void TaskScope::PostImpl(std::unique_ptr<internal::Task> task) {
  std::scoped_lock lock{checker_};
  if (stopped_) {
    return;
  }
  queue_.Add(std::move(task));
}

void TaskScope::PostImpl(std::unique_ptr<DelayedTask> task, zx::duration delay) {
  zx::time deadline = async::Now(dispatcher_) + delay;
  PostImpl(std::move(task), deadline);
}

void TaskScope::PostImpl(std::unique_ptr<DelayedTask> task, zx::time deadline) {
  std::scoped_lock lock{checker_};
  if (stopped_) {
    return;
  }
  if (!task->Post(dispatcher_, deadline)) {
    return;
  }
  list_add_tail(&delayed_task_list_, task.release());
}

void TaskScope::RemoveDelayedTask(DelayedTask* task) {
  std::scoped_lock lock{checker_};
  RemoveDelayedTaskLocked(task);
}

// NOLINTNEXTLINE(readability-convert-member-functions-to-static): for thread safety annotations
void TaskScope::RemoveDelayedTaskLocked(DelayedTask* task) {
  list_delete(task);
  delete task;
}

}  // namespace async_patterns
