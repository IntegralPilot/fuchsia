// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_RUNTIME_TESTING_CPP_SYNC_HELPERS_H
#define LIB_DRIVER_RUNTIME_TESTING_CPP_SYNC_HELPERS_H

#include <lib/async/dispatcher.h>
#include <lib/driver/runtime/testing/cpp/internal/wait_for.h>
#include <lib/fit/function.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/result.h>

#include <future>

namespace fdf {

// Run |task| on the |dispatcher|, and wait until it is completed.
// This will run all of the unmanaged dispatchers with |fdf_testing_run_until_idle|,
// if there are any, to ensure the task can be completed.
//
// This MUST be called from the main test thread.
zx::result<> RunOnDispatcherSync(async_dispatcher_t* dispatcher, fit::closure task);

// Wait until the completion is signaled. When this function returns, the completion is signaled.
// This will run all of the unmanaged dispatchers with |fdf_testing_run_until_idle|,
// if there are any.
//
// This MUST be called from the main test thread.
zx::result<> WaitFor(libsync::Completion& completion);

// Wait until the future is resolved, then returns the resolved value of the future.
// This will run all of the unmanaged dispatchers with |fdf_testing_run_until_idle|,
// if there are any.
//
// This MUST be called from the main test thread.
template <typename T>
zx::result<T> WaitFor(std::shared_future<T> future) {
  using namespace std::chrono_literals;
  zx::result wait_result = fdf_internal::IfExistsRunUnmanagedUntil(
      [&] { return future.wait_for(1ms) != std::future_status::timeout; });
  if (wait_result.is_error()) {
    return wait_result.take_error();
  }

  return zx::ok(future.get());
}

// Wait until the future is resolved, then returns the resolved value of the future.
// This will run all of the unmanaged dispatchers with |fdf_testing_run_until_idle|,
// if there are any.
//
// This MUST be called from the main test thread.
template <typename T>
zx::result<T> WaitFor(std::future<T> future) {
  return WaitFor(future.share());
}

}  // namespace fdf

#endif  // LIB_DRIVER_RUNTIME_TESTING_CPP_SYNC_HELPERS_H
