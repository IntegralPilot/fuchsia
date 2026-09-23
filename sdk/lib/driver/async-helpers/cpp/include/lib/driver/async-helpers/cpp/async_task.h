// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_ASYNC_HELPERS_CPP_ASYNC_TASK_H_
#define LIB_DRIVER_ASYNC_HELPERS_CPP_ASYNC_TASK_H_

#include <lib/async/cpp/sequence_checker.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>

namespace fdf::async_helpers {

// Forward declaration.
class TaskGroup;

class [[nodiscard]] AsyncTask final {
  struct SharedState {
    explicit SharedState(bool completed) : completed(completed) {}
    async::synchronization_checker sync_checker =
        async::synchronization_checker{fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                       "async_helpers::AsyncTask is not thread-safe."};
    fit::closure on_completed = {};
    bool completed;
  };

 public:
  explicit AsyncTask(bool completed = false)
      : shared_state_(std::make_shared<SharedState>(completed)) {}

  // Returns a deferred callback that will complete this AsyncTask when it goes out of scope.
  fit::deferred_callback CreateCompleter();

  template <typename T>
  void SetItem(T item) {
    cleanup_item_ = [item = std::move(item)]() mutable {};
  }

 private:
  friend TaskGroup;
  // Used by the TaskGroup to remove this AsyncTask from its internal state when completed.
  void OnCompleted(fit::closure on_completed);

  std::shared_ptr<SharedState> shared_state_;
  fit::closure cleanup_item_;
};

}  // namespace fdf::async_helpers

#endif  // LIB_DRIVER_ASYNC_HELPERS_CPP_ASYNC_TASK_H_
