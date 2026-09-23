// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/testing/util/logging_event_loop.h"

#include <lib/syslog/cpp/macros.h>

#include "lib/async-loop/testing/cpp/real_loop.h"

namespace ui_testing {

namespace {
/// Returns a string describing the provided source location.
static std::string ToString(const cpp20::source_location& location) {
  std::string file = location.file_name() ? location.file_name() : "<unknown>";
  std::string line = std::to_string(location.line());
  std::string func = location.function_name() ? location.function_name() : "<unknown>";
  return file + ":" + line + " (" + func + ")";
}
}  // namespace

void LoggingEventLoop::RunLoop(cpp20::source_location caller) {
  FX_LOGS(INFO) << "Running loop (from " << ToString(caller) << ")";
  RealLoop::RunLoop();
}

bool LoggingEventLoop::RunLoopWithTimeout(zx::duration timeout, cpp20::source_location caller) {
  FX_LOGS(INFO) << "Running until timeout (from " << ToString(caller) << ")";
  return RealLoop::RunLoopWithTimeout(timeout);
}

void LoggingEventLoop::RunLoopUntil(fit::function<bool()> condition,
                                    cpp20::source_location caller) {
  FX_LOGS(INFO) << "Waiting for condition from " << ToString(caller);
  RealLoop::RunLoopUntil(std::move(condition));
}

bool LoggingEventLoop::RunLoopWithTimeoutOrUntil(fit::function<bool()> condition,
                                                 zx::duration timeout, zx::duration step,
                                                 cpp20::source_location caller) {
  FX_LOGS(INFO) << "Waiting for condition or timeout from " << ToString(caller);
  return RealLoop::RunLoopWithTimeoutOrUntil(std::move(condition), timeout);
}

void LoggingEventLoop::RunLoopUntilIdle(cpp20::source_location caller) {
  FX_LOGS(INFO) << "Running until idle (from " << ToString(caller) << ")";
  RealLoop::RunLoopUntilIdle();
}

async_dispatcher_t* LoggingEventLoop::dispatcher() { return RealLoop::dispatcher(); }

}  // namespace ui_testing
