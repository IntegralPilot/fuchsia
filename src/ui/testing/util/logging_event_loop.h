// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTING_UTIL_LOGGING_EVENT_LOOP_H_
#define SRC_UI_TESTING_UTIL_LOGGING_EVENT_LOOP_H_

#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/source_location.h>
#include <lib/zx/time.h>

namespace ui_testing {

/// `LoggingEventLoop` provides a subset of the `RealLoop` API, and adds logging.
/// Feel free to expose more of the `RealLoop` API as needed.
///
/// Note: the Google C++ style guide requires `public` inheritance. But
/// 1. Public inheritance would allow callers to skip logging (by casting to the base
///    class), and/or if this class doesn't override all the overloads of the base
///    class (the overloads are complicated by `default` parameters), and
/// 2. Composition won't work here, because the methods that `LoggingEventLoop`
///    needs to call are `protected` in the base class.
///
/// It _is_ possible to meet this needs while satisfying the style guide. A helper
/// class could inherit from `RealLoop`, and `LoggingEventLoop` could instantiate that
/// class and forward calls to it. But that adds more code without a meaningful benefit.
class LoggingEventLoop : private loop_fixture::RealLoop {
 public:
  LoggingEventLoop() = default;
  ~LoggingEventLoop() = default;

  void RunLoop(cpp20::source_location caller = cpp20::source_location::current());

  void RunLoopUntil(fit::function<bool()> condition,
                    cpp20::source_location caller = cpp20::source_location::current());

  bool RunLoopWithTimeout(zx::duration timeout,
                          cpp20::source_location caller = cpp20::source_location::current());

  bool RunLoopWithTimeoutOrUntil(fit::function<bool()> condition, zx::duration timeout,
                                 zx::duration step = zx::msec(10),
                                 cpp20::source_location caller = cpp20::source_location::current());

  void RunLoopUntilIdle(cpp20::source_location caller = cpp20::source_location::current());

  async_dispatcher_t* dispatcher();
};

}  // namespace ui_testing

#endif  // SRC_UI_TESTING_UTIL_LOGGING_EVENT_LOOP_H_
