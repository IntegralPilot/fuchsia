// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/env.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>

#include <zxtest/zxtest.h>

#include "src/devices/bin/driver_runtime/dispatcher.h"
#include "src/devices/bin/driver_runtime/dispatcher_coordinator.h"
#include "src/devices/bin/driver_runtime/test_utils.h"
#include "src/devices/bin/driver_runtime/thread_context.h"

namespace driver_runtime {
extern DispatcherCoordinator& GetDispatcherCoordinator();
}

TEST(DispatcherDeathTest, DestroyAllDispatchersCrashesIfInObserver) {
  test_utils::RunWithLsanDisabled([&] {
    ASSERT_EQ(ZX_OK, driver_runtime::GetDispatcherCoordinator().Start(0));

    uint32_t fake_driver = 0;
    thread_context::PushDriver(&fake_driver);
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    libsync::Completion entered_observer;
    libsync::Completion complete_observer;

    // This dispatcher will block in its shutdown observer callback.
    auto dispatcher = fdf::SynchronizedDispatcher::Create(
        {}, "",
        [&](fdf_dispatcher_t* dispatcher) {
          entered_observer.Signal();
          complete_observer.Wait();
        },
        "");
    ASSERT_OK(dispatcher.status_value());

    // This will start the shutdown, but the runtime thread will be blocked
    // on the shutdown observer callback.
    dispatcher->ShutdownAsync();
    // Check the observer is called.
    entered_observer.Wait();

    // This should crash as we are still running the dispatcher shutdown observer callback.
    ASSERT_DEATH([&] { fdf_env_destroy_all_dispatchers(); });

    complete_observer.Signal();
    // The crash means we cannot properly destruct the dispatcher.
    dispatcher->release();
  });
}
