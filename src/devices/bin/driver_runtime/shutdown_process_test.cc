// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/channel.h>
#include <lib/fdf/cpp/channel_read.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fdf/testing.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/event.h>
#include <zircon/errors.h>

#include <zxtest/zxtest.h>

#include "lib/fdf/dispatcher.h"
#include "src/devices/bin/driver_runtime/dispatcher.h"
#include "src/devices/bin/driver_runtime/dispatcher_coordinator.h"
#include "src/devices/bin/driver_runtime/runtime_test_case.h"
#include "src/devices/bin/driver_runtime/thread_context.h"

// This is done separately to the DispatcherTest as it shuts down the process
// wide async loop backing the dispatcher, and that can't be restarted.
class ShutdownProcessTest : public RuntimeTestCase {
 protected:
  fdf_testing::DriverRuntime runtime;
};

// Tests shutting down the process async loop while requests are still pending.
TEST_F(ShutdownProcessTest, ShutdownProcessAsyncLoop) {
  const void* driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto shutdown_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      driver, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "", shutdown_handler);
  ASSERT_FALSE(dispatcher.is_error());

  auto channels = fdf::ChannelPair::Create(0);
  ASSERT_OK(channels.status_value());
  auto local = std::move(channels->end0);
  auto remote = std::move(channels->end1);

  libsync::Completion entered_read;
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      local.get(), 0,
      [&](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read, zx_status_t status) {
        entered_read.Signal();
        // Do not let the read callback complete until the loop has entered a shutdown state.
        auto* loop = driver_runtime::GetDispatcherCoordinator().default_thread_pool()->loop();
        while (loop->GetState() != ASYNC_LOOP_SHUTDOWN) {
        }
      });
  ASSERT_OK(channel_read->Begin(dispatcher->get()));

  // The call is reentrant so the read will be queued on the async loop.
  ASSERT_OK(fdf_testing_set_default_dispatcher(dispatcher->get()));
  ASSERT_EQ(ZX_OK, fdf_channel_write(remote.get(), 0, nullptr, nullptr, 0, nullptr, 0));
  // This will queue the wait to run |Dispatcher::CompleteShutdown|.
  dispatcher->ShutdownAsync();

  ASSERT_OK(entered_read.Wait(zx::time::infinite()));

  driver_runtime::GetDispatcherCoordinator().default_thread_pool()->loop()->Shutdown();

  shutdown_completion.Wait();
  ASSERT_OK(fdf_testing_set_default_dispatcher(nullptr));
}
