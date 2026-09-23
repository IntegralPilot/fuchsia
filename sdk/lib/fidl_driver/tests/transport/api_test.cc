// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.transport/cpp/driver/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/runtime/testing/cpp/sync_helpers.h>
#include <lib/sync/cpp/completion.h>

#include <thread>

#include <gtest/gtest.h>

#include "sdk/lib/fidl_driver/tests/transport/api_test_helper.h"
#include "sdk/lib/fidl_driver/tests/transport/scoped_fake_driver.h"
#include "src/lib/testing/predicates/status.h"

// Test creating a typed channel endpoint pair.
TEST(Endpoints, CreateFromProtocol) {
  // `std::move` pattern
  {
    auto endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
    ASSERT_OK(endpoints.status_value());
    ASSERT_EQ(ZX_OK, endpoints.status_value());
    fdf::ClientEnd<test_transport::TwoWayTest> client_end = std::move(endpoints->client);
    fdf::ServerEnd<test_transport::TwoWayTest> server_end = std::move(endpoints->server);

    ASSERT_TRUE(client_end.is_valid());
    ASSERT_TRUE(server_end.is_valid());
  }

  // Destructuring pattern
  {
    auto endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
    ASSERT_OK(endpoints.status_value());
    ASSERT_EQ(ZX_OK, endpoints.status_value());
    auto [client_end, server_end] = std::move(endpoints.value());

    ASSERT_TRUE(client_end.is_valid());
    ASSERT_TRUE(server_end.is_valid());
  }
}

// Test creating a typed channel endpoint pair using the out-parameter
// overloads.
TEST(Endpoints, CreateFromProtocolOutParameterStyleClientRetained) {
  fdf::ClientEnd<test_transport::TwoWayTest> client_end;
  auto server_end = fdf::CreateEndpoints(&client_end);
  ASSERT_OK(server_end.status_value());
  ASSERT_EQ(ZX_OK, server_end.status_value());

  ASSERT_TRUE(client_end.is_valid());
  ASSERT_TRUE(server_end->is_valid());
}

TEST(Endpoints, CreateFromProtocolOutParameterStyleServerRetained) {
  fdf::ServerEnd<test_transport::TwoWayTest> server_end;
  auto client_end = fdf::CreateEndpoints(&server_end);
  ASSERT_OK(client_end.status_value());
  ASSERT_EQ(ZX_OK, client_end.status_value());

  ASSERT_TRUE(server_end.is_valid());
  ASSERT_TRUE(client_end->is_valid());
}

TEST(WireClient, CannotDestroyInDifferentDispatcherThanBound) {
  DEBUG_ONLY_TEST_MAY_SKIP();
  fidl_driver_testing::ScopedFakeDriver driver;

  auto [dispatcher1, dispatcher1_shutdown] = CreateSyncDispatcher();
  auto [dispatcher2, dispatcher2_shutdown] = CreateSyncDispatcher();
  zx::result endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  std::unique_ptr<fdf::WireClient<test_transport::TwoWayTest>> client;

  // Create on one.
  libsync::Completion created;
  async::PostTask(dispatcher1.async_dispatcher(), [&, dispatcher = dispatcher1.get()] {
    client = std::make_unique<fdf::WireClient<test_transport::TwoWayTest>>();
    client->Bind(std::move(endpoints->client), dispatcher);
    created.Signal();
  });
  created.Wait();

  // Destroy on another.
  libsync::Completion destroyed;
  async::PostTask(dispatcher2.async_dispatcher(), [&] {
    ASSERT_DEATH(client.reset(),
                 "The selected FIDL bindings is thread unsafe\\. Access from multiple driver "
                 "dispatchers detected\\. This is not allowed\\. Ensure the object is used from "
                 "the same \\|fdf_dispatcher_t\\|.");
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
  fidl_driver_testing::ScopedFakeDriver driver;

  auto [dispatcher1, dispatcher1_shutdown] = CreateSyncDispatcher();
  zx::result endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  std::unique_ptr<fdf::WireClient<test_transport::TwoWayTest>> client;

  // Create on one.
  libsync::Completion created;
  async::PostTask(dispatcher1.async_dispatcher(), [&, dispatcher = dispatcher1.get()] {
    client = std::make_unique<fdf::WireClient<test_transport::TwoWayTest>>();
    client->Bind(std::move(endpoints->client), dispatcher);
    created.Signal();
  });
  created.Wait();

  // Destroy on another.
  libsync::Completion destroyed;
  std::thread thread([&] {
    ASSERT_DEATH(
        client.reset(),
        "The selected FIDL bindings is thread unsafe\\. The current thread is not managed by a "
        "driver dispatcher\\. Ensure the object is always used from "
        "a dispatcher managed thread\\.");
    destroyed.Signal();
  });
  destroyed.Wait();
  thread.join();

  dispatcher1.ShutdownAsync();
  dispatcher1_shutdown->Wait();
}

using DispatcherOptions = uint32_t;

// Test the shared client using both synchronized and unsynchronized dispatcher.
class WireSharedClient : public testing::TestWithParam<DispatcherOptions> {};

TEST_P(WireSharedClient, CanSendAcrossDispatcher) {
  fidl_driver_testing::ScopedFakeDriver driver;

  auto [dispatcher1, dispatcher1_shutdown] = CreateSyncDispatcher();
  auto [dispatcher2, dispatcher2_shutdown] = CreateSyncDispatcher();
  zx::result endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  std::unique_ptr<fdf::WireSharedClient<test_transport::TwoWayTest>> client;

  // Create on one.
  libsync::Completion created;
  async::PostTask(dispatcher1.async_dispatcher(), [&, dispatcher = dispatcher1.get()] {
    client = std::make_unique<fdf::WireSharedClient<test_transport::TwoWayTest>>();
    client->Bind(std::move(endpoints->client), dispatcher);
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
  fidl_driver_testing::ScopedFakeDriver driver;

  auto [dispatcher1, dispatcher1_shutdown] = CreateSyncDispatcher();
  zx::result endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  std::unique_ptr<fdf::WireSharedClient<test_transport::TwoWayTest>> client;

  // Create on one.
  libsync::Completion created;
  libsync::Completion destroyed;
  async::PostTask(dispatcher1.async_dispatcher(), [&, dispatcher = dispatcher1.get()] {
    client = std::make_unique<fdf::WireSharedClient<test_transport::TwoWayTest>>();
    client->Bind(std::move(endpoints->client), dispatcher,
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
  fidl_driver_testing::ScopedFakeDriver driver;

  libsync::Completion dispatcher_shutdown;
  zx::result dispatcher = fdf::UnsynchronizedDispatcher::Create(
      {}, "", [&](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown.Signal(); });
  ASSERT_OK(dispatcher.status_value());

  zx::result endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  fdf::WireClient<test_transport::TwoWayTest> client;
  libsync::Completion created;
  async::PostTask(dispatcher->async_dispatcher(), [&] {
    ASSERT_DEATH(client.Bind(std::move(endpoints->client), dispatcher->get()),
                 "The selected FIDL bindings is thread unsafe\\. A synchronized fdf_dispatcher_t "
                 "is required\\. Ensure the fdf_dispatcher_t does not have the "
                 "\\|FDF_DISPATCHER_OPTION_UNSYNCHRONIZED\\| option\\.");
    client = {};
    created.Signal();
  });
  created.Wait();

  dispatcher->ShutdownAsync();
  dispatcher_shutdown.Wait();
}

TEST_P(WireSharedClient, CanBindAnyDispatcher) {
  fidl_driver_testing::ScopedFakeDriver driver;

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

  zx::result endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  fdf::WireSharedClient<test_transport::TwoWayTest> client;
  libsync::Completion created;
  async::PostTask(dispatcher->async_dispatcher(), [&] {
    client.Bind(std::move(endpoints->client), dispatcher->get());
    client = {};
    created.Signal();
  });
  created.Wait();

  dispatcher->ShutdownAsync();
  dispatcher_shutdown.Wait();
}

INSTANTIATE_TEST_SUITE_P(WireSharedClientTests, WireSharedClient,
                         testing::Values<DispatcherOptions>(FDF_DISPATCHER_OPTION_SYNCHRONIZED,
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

template <typename T>
struct ClientUnbindTest : public ::testing::Test {};
TYPED_TEST_SUITE_P(ClientUnbindTest);

TYPED_TEST_P(ClientUnbindTest, UnbindMaybeGetEndpoint) {
  using Client = TypeParam;

  fidl_driver_testing::ScopedFakeDriver driver;
  auto [dispatcher, dispatcher_shutdown] = CreateSyncDispatcher();
  auto endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());
  fdf_handle_t handle_number = endpoints->client.channel().get();

  zx::result result =
      fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&, dispatcher = dispatcher.get()] {
        Client client(std::move(endpoints->client), dispatcher);
        fit::result<fidl::Error, fdf::ClientEnd<test_transport::TwoWayTest>> result =
            client.UnbindMaybeGetEndpoint();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_EQ(handle_number, result.value().channel().get());
      });
  ASSERT_OK(result.status_value());

  dispatcher.ShutdownAsync();
  dispatcher_shutdown->Wait();
}

TYPED_TEST_P(ClientUnbindTest, UnbindAfterFatalError) {
  using Client = TypeParam;

  fidl_driver_testing::ScopedFakeDriver driver;
  auto [dispatcher, dispatcher_shutdown] = CreateSyncDispatcher();
  auto endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  struct EventHandler : public fdf::WireAsyncEventHandler<test_transport::TwoWayTest>,
                        public fdf::AsyncEventHandler<test_transport::TwoWayTest> {
    void on_fidl_error(fidl::UnbindInfo info) final { errored.Signal(); }
    libsync::Completion errored;
  } event_handler;
  std::optional<Client> client;
  {
    zx::result result =
        fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&, dispatcher = dispatcher.get()] {
          client.emplace(std::move(endpoints->client), dispatcher, &event_handler);
        });
    ASSERT_OK(result.status_value());
  }

  {
    zx::result result =
        fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&] { endpoints->server.reset(); });
    ASSERT_OK(result.status_value());
  }

  {
    zx::result result = fdf::WaitFor(event_handler.errored);
    ASSERT_OK(result.status_value());
  }

  {
    zx::result result = fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&] {
      fit::result result = client.value().UnbindMaybeGetEndpoint();
      ASSERT_TRUE(result.is_error());
      EXPECT_TRUE(result.error_value().is_peer_closed());

      client.reset();
    });
    ASSERT_OK(result.status_value());
  }

  dispatcher.ShutdownAsync();
  dispatcher_shutdown->Wait();
}

TYPED_TEST_P(ClientUnbindTest, UnbindTwice) {
  using Client = TypeParam;

  fidl_driver_testing::ScopedFakeDriver driver;
  auto [dispatcher, dispatcher_shutdown] = CreateSyncDispatcher();
  auto endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  zx::result result =
      fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&, dispatcher = dispatcher.get()] {
        Client client(std::move(endpoints->client), dispatcher);

        {
          fit::result result = client.UnbindMaybeGetEndpoint();
          ASSERT_TRUE(result.is_ok()) << result.error_value();
        }

        {
          fit::result result = client.UnbindMaybeGetEndpoint();
          ASSERT_TRUE(result.is_error());
          EXPECT_EQ(result.error_value().reason(), fidl::Reason::kUnbind);
        }
      });
  ASSERT_OK(result.status_value());

  dispatcher.ShutdownAsync();
  dispatcher_shutdown->Wait();
}

TYPED_TEST_P(ClientUnbindTest, UnbindWithPendingCallsShouldFail) {
  using Client = TypeParam;

  fidl_driver_testing::ScopedFakeDriver driver;
  auto [dispatcher, dispatcher_shutdown] = CreateSyncDispatcher();
  auto endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());

  zx::result result =
      fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&, dispatcher = dispatcher.get()] {
        Client client(std::move(endpoints->client), dispatcher);
        if constexpr (std::is_same_v<Client, fdf::WireClient<test_transport::TwoWayTest>>) {
          fdf::Arena arena('TEST');
          client.buffer(arena)->TwoWay({}).ThenExactlyOnce([](auto&) {});
        } else {
          client->TwoWay({}).ThenExactlyOnce([](auto&) {});
        }

        {
          fit::result result = client.UnbindMaybeGetEndpoint();
          ASSERT_TRUE(result.is_error());
          EXPECT_EQ(result.error_value().reason(), fidl::Reason::kPendingTwoWayCallPreventsUnbind);
        }

        {
          fit::result result = client.UnbindMaybeGetEndpoint();
          ASSERT_TRUE(result.is_error());
          EXPECT_EQ(result.error_value().reason(), fidl::Reason::kUnbind);
        }
      });
  ASSERT_OK(result.status_value());

  dispatcher.ShutdownAsync();
  dispatcher_shutdown->Wait();
}

TYPED_TEST_P(ClientUnbindTest, UnbindAfterACompletedTwoWayCall) {
  using Client = TypeParam;

  fidl_driver_testing::ScopedFakeDriver driver;
  auto [dispatcher, dispatcher_shutdown] = CreateSyncDispatcher();
  auto endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_OK(endpoints.status_value());
  fdf_handle_t handle_number = endpoints->client.channel().get();

  libsync::Completion call_done;
  std::optional<Client> client;
  {
    zx::result result =
        fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&, dispatcher = dispatcher.get()] {
          client.emplace(std::move(endpoints->client), dispatcher);
          if constexpr (std::is_same_v<Client, fdf::WireClient<test_transport::TwoWayTest>>) {
            fdf::Arena arena('TEST');
            client.value().buffer(arena)->TwoWay({}).ThenExactlyOnce(
                [&](auto&) { call_done.Signal(); });
          } else {
            client.value()->TwoWay({}).ThenExactlyOnce([&](auto&) { call_done.Signal(); });
          }
        });
    ASSERT_OK(result.status_value());
  }

  class Server : public fdf::Server<test_transport::TwoWayTest> {
   public:
    void TwoWay(TwoWayRequest& request, TwoWayCompleter::Sync& completer) final {
      completer.Reply({request.payload()});
    }
  } server;
  std::optional<fdf::ServerBinding<test_transport::TwoWayTest>> binding;

  {
    zx::result result =
        fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&, dispatcher = dispatcher.get()] {
          binding.emplace(dispatcher, std::move(endpoints->server), &server,
                          fidl::kIgnoreBindingClosure);
        });
    ASSERT_OK(result.status_value());
  }

  ASSERT_OK(fdf::WaitFor(call_done).status_value());

  {
    zx::result result = fdf::RunOnDispatcherSync(dispatcher.async_dispatcher(), [&] {
      fit::result result = (*client).UnbindMaybeGetEndpoint();
      ASSERT_TRUE(result.is_ok()) << result.error_value();
      ASSERT_EQ(handle_number, result.value().channel().get());

      client.reset();
      binding.reset();
    });
    ASSERT_OK(result.status_value());
  }

  dispatcher.ShutdownAsync();
  dispatcher_shutdown->Wait();
}

REGISTER_TYPED_TEST_SUITE_P(ClientUnbindTest, UnbindMaybeGetEndpoint, UnbindAfterFatalError,
                            UnbindTwice, UnbindWithPendingCallsShouldFail,
                            UnbindAfterACompletedTwoWayCall);
using ClientTypes = testing::Types<fdf::Client<test_transport::TwoWayTest>,
                                   fdf::WireClient<test_transport::TwoWayTest>>;
INSTANTIATE_TYPED_TEST_SUITE_P(TestPrefix, ClientUnbindTest, ClientTypes);
