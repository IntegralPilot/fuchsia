// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/task.h>
#include <lib/driver/runtime/testing/cpp/sync_helpers.h>

namespace fdf {

zx::result<> RunOnDispatcherSync(async_dispatcher_t* dispatcher, fit::closure task) {
  libsync::Completion task_completion;
  async::PostTask(dispatcher, [task = std::move(task), &task_completion]() {
    task();
    task_completion.Signal();
  });

  return WaitFor(task_completion);
}

zx::result<> WaitFor(libsync::Completion& completion) {
  zx::result wait_result =
      fdf_internal::IfExistsRunUnmanagedUntil([&] { return completion.signaled(); });
  if (wait_result.is_error()) {
    return wait_result.take_error();
  }

  completion.Wait();
  return zx::ok();
}

}  // namespace fdf
