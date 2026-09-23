// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_TESTS_UTILS_BLOCKING_PRESENT_H_
#define SRC_UI_SCENIC_TESTS_UTILS_BLOCKING_PRESENT_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/stdcompat/source_location.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"
#include "src/ui/testing/util/logging_event_loop.h"

namespace integration_tests {

// Invokes `flatland->Present()` and then uses `loop` to loop until
// 1. Scenic indicates that the frame has been presented.
// 2. Scenic indicates that the flatland client can begin rendering the next frame.
//
// Note: temporarily sets `Flatland` event handlers for `OnFramePresented` and `OnNextFrameBegin`,
// and resets them afterward.
void BlockingPresent(ui_testing::LoggingEventLoop* loop, FlatlandClientWithEventHandler& flatland,
                     fuchsia_ui_composition::PresentArgs present_args = {},
                     cpp20::source_location = cpp20::source_location::current());

}  // namespace integration_tests

#endif  // SRC_UI_SCENIC_TESTS_UTILS_BLOCKING_PRESENT_H_
