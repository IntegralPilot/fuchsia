// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/async-helpers/cpp/task_group.h>

#include <mutex>

namespace fdf::async_helpers {

void TaskGroup::AddTask(AsyncTask async_task) {
  std::lock_guard guard(sync_checker_);
  outstanding_work_[current_key_] = std::move(async_task);
  outstanding_work_[current_key_].OnCompleted(
      [this, key = current_key_]() { outstanding_work_.erase(key); });
  current_key_++;
}

}  // namespace fdf::async_helpers
