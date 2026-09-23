// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/time.h>
#include <lib/async_patterns/cpp/internal/task_queue.h>
#include <lib/sync/cpp/mutex.h>
#include <threads.h>
#include <zircon/assert.h>

#include <mutex>

namespace async_patterns::internal {

std::shared_ptr<TaskQueue> TaskQueue::Create(async_dispatcher_t* dispatcher,
                                             const char* description) {
  return std::make_shared<TaskQueue>(dispatcher, description);
}

TaskQueue::TaskQueue(async_dispatcher_t* dispatcher, const char* description)
    : async_task_t({{ASYNC_STATE_INIT},
                    [](async_dispatcher_t*, async_task_t* task, zx_status_t status) {
                      static_cast<TaskQueue*>(task)->OnWake(status);
                    },
                    async_now(dispatcher)}),
      dispatcher_(dispatcher),
      checker_(dispatcher, description) {
  std::lock_guard guard(checker_);
  ZX_DEBUG_ASSERT(dispatcher_ != nullptr);
}

TaskQueue::~TaskQueue() {
  std::lock_guard guard(mutex_);
  ZX_ASSERT(stopped_);
  ZX_ASSERT(!wake_pending_);
  ZX_ASSERT(list_is_empty(&task_list_));
}

void TaskQueue::Add(std::unique_ptr<Task> task) {
  list_node_t tasks;
  bool ok;
  {
    std::lock_guard guard(mutex_);
    if (stopped_) {
      return;
    }
    list_add_tail(&task_list_, task.release());
    ok = WakeDispatcher();
    if (!ok) {
      StopLocked();
      list_move(&task_list_, &tasks);
    }
  }
  if (!ok) {
    DropTasks(&tasks);
  }
}

void TaskQueue::Stop() {
  std::lock_guard guard(checker_);
  list_node_t tasks;
  {
    std::lock_guard mutex_guard(mutex_);
    if (stopped_) {
      return;
    }
    StopLocked();
    list_move(&task_list_, &tasks);
  }
  DropTasks(&tasks);
}

void TaskQueue::OnWake(zx_status_t status) {
  std::lock_guard outer(checker_);
  list_node_t tasks;

  // Harvest tasks under the lock.
  {
    std::lock_guard inner(mutex_);
    wake_pending_ = false;
    if (status != ZX_OK) {
      StopLocked();
    }
    list_move(&task_list_, &tasks);
  }

  if (status != ZX_OK) {
    DropTasks(&tasks);
  } else {
    RunTasks(&tasks);
  }
}

void TaskQueue::StopLocked() {
  ZX_DEBUG_ASSERT(!stopped_);
  stopped_ = true;
  CancelWakeDispatcher();
}

void TaskQueue::DropTasks(list_node_t* tasks) {
  list_node_t* node = nullptr;
  list_node_t* temp_node = nullptr;
  list_for_every_safe(tasks, node, temp_node) {
    list_delete(node);
    auto* task = static_cast<Task*>(node);
    delete task;
  }
}

void TaskQueue::RunTasks(list_node_t* tasks) {
  list_node_t* node = nullptr;
  list_node_t* temp_node = nullptr;
  list_for_every_safe(tasks, node, temp_node) {
    list_delete(node);
    auto* task = static_cast<Task*>(node);
    task->Run();
    delete task;
  }
}

bool TaskQueue::WakeDispatcher() {
  if (wake_pending_) {
    return true;
  }
  zx_status_t status = async_post_task(dispatcher_, this);
  ZX_DEBUG_ASSERT(status == ZX_OK || status == ZX_ERR_BAD_STATE);
  return wake_pending_ = status == ZX_OK;
}

void TaskQueue::CancelWakeDispatcher() {
  if (!wake_pending_) {
    return;
  }
  zx_status_t status = async_cancel_task(dispatcher_, this);
  ZX_DEBUG_ASSERT(status == ZX_OK);
  wake_pending_ = false;
}

}  // namespace async_patterns::internal
