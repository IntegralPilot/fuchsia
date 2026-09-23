// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.empty.protocol/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/env.h>
#include <lib/sync/cpp/completion.h>

#include <utility>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

#ifdef NDEBUG
#define DEBUG_ONLY_TEST_MAY_SKIP() GTEST_SKIP() << "Skipped in release build"
#else
#define DEBUG_ONLY_TEST_MAY_SKIP() (void)0
#endif

// These tests verify the operation of `fidl::` clients (i.e. Zircon channel
// transport) over driver async dispatchers. Driver async dispatchers may
// use multiple Zircon threads with synchronization (no two threads run the
// dispatcher at the same time), and FIDL clients should work over those.
//
// Because death tests are involved, and zxtest death tests are run under
// new threads which don't have associated driver dispatchers, these test
// use gtest for now.

namespace {

class ScopedFakeDriver {
 public:
  ScopedFakeDriver() {
    void* driver = reinterpret_cast<void*>(1);
    fdf_env_register_driver_entry(driver);
  }

  ~ScopedFakeDriver() { fdf_env_register_driver_exit(); }
};

std::pair<fdf::Dispatcher, std::shared_ptr<libsync::Completion>> CreateSyncDispatcher() {
  // Use |FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS| to encourage the driver dispatcher
  // to spawn more threads to back the same synchronized dispatcher.
  constexpr auto kSyncDispatcherOptions = fdf::SynchronizedDispatcher::Options::kAllowSyncCalls;
  std::shared_ptr<libsync::Completion> dispatcher_shutdown =
      std::make_shared<libsync::Completion>();
  zx::result dispatcher = fdf::SynchronizedDispatcher::Create(
      kSyncDispatcherOptions, "",
      [dispatcher_shutdown](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown->Signal(); });
  ZX_ASSERT(dispatcher.status_value() == ZX_OK);
  return std::make_pair(std::move(*dispatcher), std::move(dispatcher_shutdown));
}

TEST(WireClient, CannotDestroyInDifferentDispatcherThanBound) {
  DEBUG_ONLY_TEST_MAY_SKIP();
  ScopedFakeDriver driver;

  auto [dispatcher1, dispatcher1_shutdown] = CreateSyncDispatcher();
  auto [dispatcher2, dispatcher2_shutdown] = CreateSyncDispatcher();
  auto endpoints = fidl::Endpoints<test_empty_protocol::Empty>::Create();

  std::unique_ptr<fidl::WireClient<test_empty_protocol::Empty>> client;

  // Create on one.
  libsync::Completion created;
  async::PostTask(dispatcher1.async_dispatcher(),
                  [&, async_dispatcher = dispatcher1.async_dispatcher()] {
                    client = std::make_unique<fidl::WireClient<test_empty_protocol::Empty>>();
                    client->Bind(std::move(endpoints.client), async_dispatcher);
                    created.Signal();
                  });
  created.Wait();

  // Destroy on another.
  libsync::Completion destroyed;
  async::PostTask(dispatcher2.async_dispatcher(), [&] {
    ASSERT_DEATH(client.reset(),
                 "The selected FIDL bindings is thread unsafe. Access from multiple driver "
                 "dispatchers detected. This is not allowed. Ensure the object is used from the "
                 "same |fdf_dispatcher_t|.");
    destroyed.Signal();
  });
  destroyed.Wait();

  dispatcher1.ShutdownAsync();
  dispatcher2.ShutdownAsync();

  dispatcher1_shutdown->Wait();
  dispatcher2_shutdown->Wait();
}

TEST(WireClient, CannotDestroyOnUnmanagedThread) {
  DEBUG_ONLY_TEST_MAY_SKIP();
  ScopedFakeDriver driver;

  auto [dispatcher1, dispatcher1_shutdown] = CreateSyncDispatcher();
  zx::result endpoints = fidl::CreateEndpoints<test_empty_protocol::Empty>();
  ASSERT_OK(endpoints.status_value());

  std::unique_ptr<fidl::WireClient<test_empty_protocol::Empty>> client;

  // Create on one.
  libsync::Completion created;
  async::PostTask(dispatcher1.async_dispatcher(),
                  [&, async_dispatcher = dispatcher1.async_dispatcher()] {
                    client = std::make_unique<fidl::WireClient<test_empty_protocol::Empty>>();
                    client->Bind(std::move(endpoints->client), async_dispatcher);
                    created.Signal();
                  });
  created.Wait();

  // Destroy on another.
  libsync::Completion destroyed;
  std::thread thread([&] {
    ASSERT_DEATH(
        client.reset(),
        "The selected FIDL bindings is thread unsafe. The current thread is not managed by a "
        "driver dispatcher. Ensure the object is always used from a dispatcher managed thread.");
    destroyed.Signal();
  });
  destroyed.Wait();
  thread.join();

  dispatcher1.ShutdownAsync();
  dispatcher1_shutdown->Wait();
}

using DispatcherOptions = uint32_t;

// Test the shared client using both synchronized and unsynchronized dispatcher.
class WireSharedClient : public ::testing::TestWithParam<DispatcherOptions> {};

TEST_P(WireSharedClient, CanSendAcrossDispatcher) {
  ScopedFakeDriver driver;

  auto [dispatcher1, dispatcher1_shutdown] = CreateSyncDispatcher();
  auto [dispatcher2, dispatcher2_shutdown] = CreateSyncDispatcher();
  auto endpoints = fidl::Endpoints<test_empty_protocol::Empty>::Create();

  std::unique_ptr<fidl::WireSharedClient<test_empty_protocol::Empty>> client;

  // Create on one.
  libsync::Completion created;
  async::PostTask(dispatcher1.async_dispatcher(),
                  [&, async_dispatcher = dispatcher1.async_dispatcher()] {
                    client = std::make_unique<fidl::WireSharedClient<test_empty_protocol::Empty>>();
                    client->Bind(std::move(endpoints.client), async_dispatcher);
                    created.Signal();
                  });
  created.Wait();

  // Destroy on another.
  libsync::Completion destroyed;
  async::PostTask(dispatcher2.async_dispatcher(), [&] {
    client.reset();
    destroyed.Signal();
  });
  destroyed.Wait();

  dispatcher1.ShutdownAsync();
  dispatcher2.ShutdownAsync();
  dispatcher1_shutdown->Wait();
  dispatcher2_shutdown->Wait();
}

TEST_P(WireSharedClient, CanDestroyOnUnmanagedThread) {
  ScopedFakeDriver driver;

  auto [dispatcher1, dispatcher1_shutdown] = CreateSyncDispatcher();
  zx::result endpoints = fidl::CreateEndpoints<test_empty_protocol::Empty>();
  ASSERT_OK(endpoints.status_value());

  std::unique_ptr<fidl::WireSharedClient<test_empty_protocol::Empty>> client;

  // Create on one.
  libsync::Completion created;
  libsync::Completion destroyed;
  async::PostTask(dispatcher1.async_dispatcher(),
                  [&, async_dispatcher = dispatcher1.async_dispatcher()] {
                    client = std::make_unique<fidl::WireSharedClient<test_empty_protocol::Empty>>();
                    client->Bind(std::move(endpoints->client), async_dispatcher,
                                 fidl::ObserveTeardown([&] { destroyed.Signal(); }));
                    created.Signal();
                  });
  created.Wait();

  // Destroy on another.
  std::thread thread([&] { client.reset(); });
  destroyed.Wait();
  thread.join();

  dispatcher1.ShutdownAsync();
  dispatcher1_shutdown->Wait();
}

TEST(WireClient, CannotBindUnsynchronizedDispatcher) {
  DEBUG_ONLY_TEST_MAY_SKIP();
  ScopedFakeDriver driver;

  libsync::Completion dispatcher_shutdown;
  zx::result dispatcher = fdf::UnsynchronizedDispatcher::Create(
      {}, "", [&](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown.Signal(); });
  ASSERT_OK(dispatcher.status_value());

  auto endpoints = fidl::Endpoints<test_empty_protocol::Empty>::Create();

  fidl::WireClient<test_empty_protocol::Empty> client;
  libsync::Completion created;
  async::PostTask(dispatcher->async_dispatcher(), [&] {
    ASSERT_DEATH(client.Bind(std::move(endpoints.client), dispatcher->async_dispatcher()),
                 "The selected FIDL bindings is thread unsafe. A synchronized fdf_dispatcher_t is "
                 "required. Ensure the fdf_dispatcher_t does not have the "
                 "|FDF_DISPATCHER_OPTION_UNSYNCHRONIZED| option.");
    client = {};
    created.Signal();
  });
  created.Wait();

  dispatcher->ShutdownAsync();
  dispatcher_shutdown.Wait();
}

TEST_P(WireSharedClient, CanBindAnyDispatcher) {
  ScopedFakeDriver driver;

  libsync::Completion dispatcher_shutdown;
  zx::result<fdf::Dispatcher> dispatcher;
  uint32_t options = GetParam();
  if ((options & FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK) ==
      FDF_DISPATCHER_OPTION_SYNCHRONIZED) {
    dispatcher = fdf::SynchronizedDispatcher::Create(
        fdf::SynchronizedDispatcher::Options{options}, "",
        [&](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown.Signal(); });
  } else {
    dispatcher = fdf::UnsynchronizedDispatcher::Create(
        {}, "", [&](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown.Signal(); });
  }
  ASSERT_OK(dispatcher.status_value());

  zx::result endpoints = fidl::CreateEndpoints<test_empty_protocol::Empty>();
  ASSERT_OK(endpoints.status_value());

  fidl::WireSharedClient<test_empty_protocol::Empty> client;
  libsync::Completion created;
  async::PostTask(dispatcher->async_dispatcher(), [&] {
    client.Bind(std::move(endpoints->client), dispatcher->async_dispatcher());
    client = {};
    created.Signal();
  });
  created.Wait();

  dispatcher->ShutdownAsync();
  dispatcher_shutdown.Wait();
}

INSTANTIATE_TEST_SUITE_P(WireSharedClientTests, WireSharedClient,
                         ::testing::Values<DispatcherOptions>(FDF_DISPATCHER_OPTION_SYNCHRONIZED,
                                                              FDF_DISPATCHER_OPTION_UNSYNCHRONIZED),
                         [](const auto info) {
                           switch (info.index) {
                             case 0:
                               return "Synchronized";
                             case 1:
                               return "Unsynchronized";
                             default:
                               ZX_PANIC("Invalid index");
                           }
                         });

}  // namespace
