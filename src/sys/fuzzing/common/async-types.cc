// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/async-types.h"

#include <memory>

namespace fuzzing {

ExecutorPtr MakeExecutor(async_dispatcher_t* dispatcher) {
  return std::make_shared<async::Executor>(dispatcher);
}

ZxResult<> AsZxResult(zx_status_t status) {
  if (status != ZX_OK) {
    return fpromise::error(status);
  }
  return fpromise::ok();
}

ZxResult<> AsZxResult(const Result<zx_status_t>& result) {
  if (result.is_error()) {
    return fpromise::error(ZX_ERR_INTERNAL);
  }
  return AsZxResult(result.value());
}

ZxPromise<> AwaitConsumer(Consumer<> consumer) {
  return consumer.promise_or(fpromise::error()).or_else([]() {
    return fpromise::error(ZX_ERR_CANCELED);
  });
}

ZxPromise<> AwaitConsumer(Consumer<zx_status_t> consumer) {
  return consumer.promise_or(fpromise::error()).then([](const Result<zx_status_t>& result) {
    return AsZxResult(result);
  });
}

ZxPromise<> AwaitConsumer(ZxConsumer<> consumer) {
  return consumer.promise_or(fpromise::error(ZX_ERR_CANCELED));
}

ZxPromise<> ConsumeBridge(Bridge<>& bridge) { return AwaitConsumer(std::move(bridge.consumer)); }

ZxPromise<> ConsumeBridge(Bridge<zx_status_t>& bridge) {
  return AwaitConsumer(std::move(bridge.consumer));
}

}  // namespace fuzzing
