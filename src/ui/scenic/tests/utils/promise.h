// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_TESTS_UTILS_PROMISE_H_
#define SRC_UI_SCENIC_TESTS_UTILS_PROMISE_H_

#include <lib/async-testing/test_loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/dispatcher.h>
#include <lib/fpromise/promise.h>

namespace integration_tests {

// Runs the given promise and waits for it to complete.
// Return true if result is_ok().
bool RunPromise(async_dispatcher_t* dispatcher, fit::function<void()> run,
                fpromise::promise<> promise);

// Runs the given promise on a new executor and waits for it to complete.
// Return true if result is_ok().
template <typename Loop>
inline bool RunPromise(Loop& loop, fpromise::promise<> promise) {
  return integration_tests::RunPromise(
      loop.dispatcher(), [&loop] { loop.RunUntilIdle(); }, std::move(promise));
}

// Runs the given promise on an executor and waits for it to complete.
// Return true if result is_ok().
bool RunPromise(async::Executor& executor, fit::function<void(bool&)> run_until,
                fpromise::promise<> promise);

// Returns a matcher for use with GMock. Specifically, this matches returns a promise that is
// already resolved with the given result.
inline auto ReturnPromise(fpromise::result<> result) {
  return [result](auto&&...) { return fpromise::make_result_promise(result); };
}

}  // namespace integration_tests

#endif  // SRC_UI_SCENIC_TESTS_UTILS_PROMISE_H_
