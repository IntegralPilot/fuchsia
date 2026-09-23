// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/task.h>
#include <lib/async/time.h>
#include <lib/async_patterns/cpp/internal/dispatcher_bound_storage.h>
#include <lib/fit/defer.h>

namespace async_patterns::internal {

namespace {

// |SelfDeletingTask| will run the wrapped closure even if the dispatcher
// shuts down after the task has been posted.
class SelfDeletingTask : public async_task_t {
 public:
  static void Post(async_dispatcher_t* dispatcher, fit::callback<void()> task) {
    auto* t = new SelfDeletingTask(dispatcher, std::move(task));
    zx_status_t status = async_post_task(dispatcher, t);
    if (status != ZX_OK) {
      if (status == ZX_ERR_BAD_STATE) {
        SelfDeletingTask::Invoke(dispatcher, t, ZX_ERR_BAD_STATE);
        return;
      }
      delete t;
      if (status == ZX_ERR_NOT_SUPPORTED) {
        ZX_PANIC("The |async_dispatcher_t| must support task posting.");
      }
    }

    ZX_ASSERT(status == ZX_OK);
  }

 private:
  SelfDeletingTask(async_dispatcher_t* dispatcher, fit::callback<void()> task)
      : async_task_t({{ASYNC_STATE_INIT}, &SelfDeletingTask::Invoke, async_now(dispatcher)}),
        task_(std::move(task)) {}

  static void Invoke(async_dispatcher_t* /*unused*/, async_task_t* task, zx_status_t status) {
    auto* self = static_cast<SelfDeletingTask*>(task);
    self->task_();
    delete self;
  }

  fit::callback<void()> task_;
};

constexpr char kDispatcherBoundThreadSafetyDescription[] =
    "|async_patterns::DispatcherBound| is meant to manage thread-unsafe asynchronous objects. "
    "Those objects should only be used from synchronized dispatchers. "
    "However, the |async_dispatcher_t*| provided is not a synchronized dispatcher.";

}  // namespace

SynchronizedBase::SynchronizedBase(async_dispatcher_t* dispatcher)
    : checker_(dispatcher, kDispatcherBoundThreadSafetyDescription) {}

DispatcherBoundStorage::~DispatcherBoundStorage() {
  ZX_ASSERT_MSG(!op_fn_, "Must call |Destruct| to destroy the dispatcher-bound object.");
}

void DispatcherBoundStorage::ConstructInternal(async_dispatcher_t* dispatcher,
                                               fit::callback<void()> task) {
  SelfDeletingTask::Post(dispatcher, std::move(task));
}

void DispatcherBoundStorage::CallInternal(async_dispatcher_t* dispatcher,
                                          fit::callback<void()> member) {
  ZX_ASSERT_MSG(op_fn_,
                "|async_patterns::DispatcherBound| must first hold an object before "
                "making calls on it.");
  SelfDeletingTask::Post(dispatcher, std::move(member));
}

void DispatcherBoundStorage::Destruct(async_dispatcher_t* dispatcher) {
  ZX_ASSERT(op_fn_);
  SelfDeletingTask::Post(dispatcher, [op_fn = std::move(op_fn_)] { op_fn(Operation::kDestruct); });
}

}  // namespace async_patterns::internal
