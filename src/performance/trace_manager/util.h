// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_UTIL_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_UTIL_H_

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/time.h>
#include <lib/fpromise/promise.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/socket.h>

#include <iosfwd>

namespace tracing {

enum class TransferStatus {
  // The transfer is complete.
  kComplete,
  // An error was detected with the provider, ignore its contribution to
  // trace output.
  kProviderError,
  // Writing of trace data to the receiver failed in an unrecoverable way.
  kWriteError,
  // The receiver of the transfer went away.
  kReceiverDead,
};

std::ostream& operator<<(std::ostream& out, TransferStatus status);

std::ostream& operator<<(std::ostream& out, fuchsia_tracing::BufferDisposition disposition);

std::ostream& operator<<(std::ostream& out, fuchsia_tracing_controller::SessionState state);

struct ResultTimedOut {};
template <typename Promise>
class timeout_continuation final {
 public:
  timeout_continuation(async::Executor& executor, Promise promise, zx::duration d)
      : executor_(executor),
        promise_(std::move(promise)),
        deadline_(async::Now(executor.dispatcher()) + d) {}

  fpromise::result<typename Promise::result_type, ResultTimedOut> operator()(
      fpromise::context& context) {
    auto res = promise_(context);
    if (res.is_pending()) {
      if (deadline_ <= async::Now(executor_.dispatcher())) {
        return fpromise::error(ResultTimedOut{});
      }
      // If a task requires multiple wakeups to complete, we shouldn't set multiple timeouts.
      if (!timeout_scheduled_) {
        executor_.schedule_task(executor_.MakePromiseForTime(deadline_).then(
            [token = context.suspend_task()](fpromise::result<void, void>&) mutable {
              token.resume_task();
            }));
        timeout_scheduled_ = true;
      }

      return fpromise::pending();
    }
    return fpromise::ok(res);
  }

 private:
  bool timeout_scheduled_{false};
  async::Executor& executor_;
  Promise promise_;
  zx::time deadline_;
};

template <typename Promise>
inline fpromise::promise_impl<timeout_continuation<Promise>> with_timeout(async::Executor& executor,
                                                                          Promise promise,
                                                                          zx::duration d) {
  return make_promise_with_continuation(
      timeout_continuation<Promise>(executor, std::move(promise), d));
}

}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_MANAGER_UTIL_H_
