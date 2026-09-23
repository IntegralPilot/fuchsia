// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/task.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fdf/env.h>
#include <lib/sync/cpp/completion.h>
#include <stdio.h>
#include <zircon/compiler.h>

#include <zxtest/zxtest.h>

__EXPORT int main(int argc, char** argv) {
  // TODO(https://fxbug.dev/42073486): Remove this once the elf runner no longer
  // fools libc into block-buffering stdout.
  setlinebuf(stdout);
  if (zx_status_t status = fdf_env_start(0); status != ZX_OK) {
    return status;
  }
  const void* driver = reinterpret_cast<void*>(0x12345678);
  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      driver, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "driver-runtime-test-main",
      [](fdf_dispatcher_t*) {});
  if (dispatcher.is_error()) {
    return dispatcher.status_value();
  }
  // create an extra dispatcher so we're guaranteed to have an extra thread for async shutdown
  // callbacks to be called on.
  auto extra_dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      driver, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "driver-runtime-test-extra",
      [](fdf_dispatcher_t*) {});
  if (extra_dispatcher.is_error()) {
    return extra_dispatcher.status_value();
  }
  extra_dispatcher->release();

  zx_status_t status;
  libsync::Completion completion;
  async::PostTask(dispatcher->async_dispatcher(), [&]() {
    status = RUN_ALL_TESTS(argc, argv);
    completion.Signal();
  });
  // Dispatcher will be destroyed by |fdf_env_destroy_all_dispatchers() below.
  dispatcher->release();
  completion.Wait();

  class Observer : public fdf_env_driver_shutdown_observer_t {
   public:
    Observer()
        : fdf_env_driver_shutdown_observer_t(
              fdf_env_driver_shutdown_observer_t{.handler = &Observer::Handler}) {}

    static void Handler(const void* driver, fdf_env_driver_shutdown_observer_t* observer) {
      static_cast<Observer*>(observer)->completion_.Signal();
    }

    void Wait() { completion_.Wait(); }

   private:
    libsync::Completion completion_;
  };
  Observer observer;
  fdf_env_shutdown_dispatchers_async(driver, &observer);
  observer.Wait();

  fdf_env_destroy_all_dispatchers();
  return status;
}
