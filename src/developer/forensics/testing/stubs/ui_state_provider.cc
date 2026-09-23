// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/ui_state_provider.h"

#include <lib/fit/defer.h>
#include <zircon/types.h>

namespace forensics::stubs {

UIStateProvider::UIStateProvider(async_dispatcher_t* dispatcher, fuchsia_ui_activity::State state,
                                 zx::time_monotonic time)
    : dispatcher_(dispatcher), state_(state), time_(time) {}

void UIStateProvider::WatchState(WatchStateRequest& request, WatchStateCompleter::Sync& completer) {
  listener_.emplace(std::move(request.listener()), dispatcher_, &event_handler_);
  OnStateChanged();
}

void UIStateProvider::SetState(fuchsia_ui_activity::State state, zx::time_monotonic time) {
  state_ = state;
  time_ = time;

  if (!IsBound() || !listener_.has_value()) {
    return;
  }

  OnStateChanged();
}

void UIStateProvider::UnbindListener() { listener_ = std::nullopt; }

void UIStateProvider::OnStateChanged() {
  auto check_callback = fit::defer(
      [] { FX_LOGS(FATAL) << "fuchsia.ui.activity/Listener.OnStateChange not responded to"; });

  (*listener_)
      ->OnStateChanged({{
          .state = state_,
          .transition_time = time_.get(),
      }})
      .ThenExactlyOnce(
          [check_callback = std::move(check_callback)](
              fidl::Result<fuchsia_ui_activity::Listener::OnStateChanged>& result) mutable {
            FX_CHECK(result.is_ok()) << "OnStateChanged failed: " << result.error_value();
            check_callback.cancel();
          });
}

}  // namespace forensics::stubs
