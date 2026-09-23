// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_PROMISE_TIMEOUT_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_PROMISE_TIMEOUT_H_

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/zx/time.h>

#include "src/developer/forensics/utils/errors.h"

namespace forensics {
namespace internal {

template <typename Promise>
class timeout_continuation final {
 public:
  explicit timeout_continuation(Promise promise, Promise timeout_promise)
      : promise_(std::move(promise)), timeout_promise_(std::move(timeout_promise)) {}

  typename Promise::result_type operator()(fpromise::context& context) {
    if (typename Promise::result_type result = promise_(context); !result.is_pending()) {
      return std::move(result);
    }

    if (typename Promise::result_type result = timeout_promise_(context); !result.is_pending()) {
      return std::move(result);
    }

    return fpromise::pending();
  }

 private:
  Promise promise_;
  Promise timeout_promise_;
};

template <typename Value>
fpromise::promise<Value, Error> make_timeout_promise(async_dispatcher_t* dispatcher,
                                                     zx::duration timeout) {
  fpromise::bridge<Value, Error> bridge;
  async::PostDelayedTask(
      dispatcher,
      [completer = std::move(bridge.completer)]() mutable {
        completer.complete_error(Error::kTimeout);
      },
      timeout);

  return bridge.consumer.promise();
}

}  // namespace internal

// Returns Error::kTimeout if |promise| does not complete within |timeout|.
template <typename Value>
fpromise::promise<Value, Error> MakePromiseTimeout(fpromise::promise<Value, Error> promise,
                                                   async_dispatcher_t* dispatcher,
                                                   const zx::duration timeout) {
  fpromise::promise<Value, Error> timeout_promise =
      internal::make_timeout_promise<Value>(dispatcher, timeout);

  return fpromise::make_promise_with_continuation(
      internal::timeout_continuation<fpromise::promise<Value, Error>>(std::move(promise),
                                                                      std::move(timeout_promise)));
}

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_PROMISE_TIMEOUT_H_
