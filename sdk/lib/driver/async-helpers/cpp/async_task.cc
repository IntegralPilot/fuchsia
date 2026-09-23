// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/async-helpers/cpp/async_task.h>

#include <mutex>

namespace fdf::async_helpers {

fit::deferred_callback AsyncTask::CreateCompleter() {
  std::weak_ptr<SharedState> weak_state = shared_state_;
  fit::callback<void()> c = [weak_state = std::move(weak_state)]() {
    std::shared_ptr<SharedState> state = weak_state.lock();
    if (state) {
      std::lock_guard lg(state->sync_checker);
      state->completed = true;
      if (state->on_completed) {
        state->on_completed();
      }
    }
  };
  return fit::defer(std::move(c));
}

void AsyncTask::OnCompleted(fit::closure on_completed) {
  std::lock_guard lg(shared_state_->sync_checker);
  if (!shared_state_->completed) {
    shared_state_->on_completed = std::move(on_completed);
  } else {
    on_completed();
  }
}

}  // namespace fdf::async_helpers
