// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_ASYNC_HELPERS_CPP_TASK_GROUP_H_
#define LIB_DRIVER_ASYNC_HELPERS_CPP_TASK_GROUP_H_

#include <lib/async/cpp/sequence_checker.h>
#include <lib/driver/async-helpers/cpp/async_task.h>
#include <lib/fdf/cpp/dispatcher.h>

namespace fdf::async_helpers {

class TaskGroup final {
 public:
  TaskGroup()
      : sync_checker_(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                      "async_helpers::TaskGroup is not thread-safe.") {}
  void AddTask(AsyncTask async_task);

 private:
  async::synchronization_checker sync_checker_;
  uint64_t current_key_ = 0;
  std::unordered_map<uint64_t, AsyncTask> outstanding_work_;
};

}  // namespace fdf::async_helpers

#endif  // LIB_DRIVER_ASYNC_HELPERS_CPP_TASK_GROUP_H_
