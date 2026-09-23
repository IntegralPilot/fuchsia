// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/tests/utils/blocking_present.h"

namespace integration_tests {

void BlockingPresent(ui_testing::LoggingEventLoop* loop, FlatlandClientWithEventHandler& flatland,
                     fuchsia_ui_composition::PresentArgs present_args,
                     cpp20::source_location caller) {
  if (!present_args.unsquashable().has_value()) {
    present_args.unsquashable(true);
  }
  // Initialize callbacks and callback state.
  bool presented = false;
  bool began = false;
  flatland.set_on_frame_presented([&presented](auto&) { presented = true; });
  flatland.set_on_next_frame_begin([&began](auto&) { began = true; });

  // Request that the current frame be presented, and wait until Scenic indicates
  // that presentation is complete.
  FX_CHECK(flatland->Present(std::move(present_args)).is_ok());
  FX_LOGS(INFO) << "Waiting for OnFramePresented";
  loop->RunLoopUntil([&presented] { return presented; }, caller);

  // Wait for `OnNextFrameBegin`. This ensures that `flatland` has present
  // credits available, and hence, the next `Present()` (if any) will not fail
  // due to `NO_PRESENTS_REMAINING`.
  FX_LOGS(INFO) << "Waiting for OnNextFrameBegin";
  loop->RunLoopUntil([&began] { return began; }, caller);

  // Reset callbacks.
  flatland.reset_on_frame_presented();
  flatland.reset_on_next_frame_begin();
}

}  // namespace integration_tests
