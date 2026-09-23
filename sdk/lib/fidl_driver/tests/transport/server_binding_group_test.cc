// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.transport/cpp/driver/wire.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/dispatcher.h>
#include <lib/sync/cpp/completion.h>

#include <memory>

#include <gtest/gtest.h>

#include "sdk/lib/fidl_driver/tests/transport/api_test_helper.h"
#include "sdk/lib/fidl_driver/tests/transport/scoped_fake_driver.h"
#include "src/lib/testing/predicates/status.h"

namespace {

TEST(ServerBindingGroup, Control) {
  fidl_driver_testing::ScopedFakeDriver driver;
  auto [dispatcher, dispatcher_shutdown] = CreateSyncDispatcher();

  struct Server : public fdf::WireServer<test_transport::TwoWayTest> {
    void TwoWay(TwoWayRequestView request, fdf::Arena& arena,
                TwoWayCompleter::Sync& completer) override {
      call_count_++;
      completer.buffer(arena).Reply(request->payload);
    }

    void Bind(fdf::ServerEnd<test_transport::TwoWayTest> server_end) {
      bindings.emplace();
      bindings->AddBinding(fdf::Dispatcher::GetCurrent()->get(), std::move(server_end), this,
                           std::mem_fn(&Server::OnClosed));
    }

    void DestroyBinding() { bindings.reset(); }

    size_t call_count() const { return call_count_; }

    bool close_handler_called() const { return close_handler_called_; }

   private:
    void OnClosed(fidl::UnbindInfo) { close_handler_called_ = true; }

    size_t call_count_ = 0;
    bool close_handler_called_ = false;
    std::optional<fdf::ServerBindingGroup<test_transport::TwoWayTest>> bindings;
  };

  auto endpoints = fdf::CreateEndpoints<test_transport::TwoWayTest>();
  ASSERT_TRUE(endpoints.is_ok()) << endpoints.status_string();
  auto [client_end, server_end] = std::move(*endpoints);

  async_patterns::TestDispatcherBound<Server> server{dispatcher.async_dispatcher(), std::in_place};
  server.AsyncCall(&Server::Bind, std::move(server_end));

  fdf::Arena arena('TEST');
  constexpr uint32_t kPayload = 42;
  EXPECT_EQ(0u, server.SyncCall(&Server::call_count));
  {
    auto result = fdf::WireCall(client_end).buffer(arena)->TwoWay(kPayload);
    ASSERT_TRUE(result.ok()) << result.error();
    EXPECT_EQ(kPayload, result->payload);
  }
  EXPECT_EQ(1u, server.SyncCall(&Server::call_count));
  {
    auto result = fdf::WireCall(client_end).buffer(arena)->TwoWay(kPayload);
    ASSERT_TRUE(result.ok()) << result.error();
    EXPECT_EQ(kPayload, result->payload);
  }
  EXPECT_EQ(2u, server.SyncCall(&Server::call_count));

  // Unbind does not call CloseHandler.
  server.SyncCall(&Server::DestroyBinding);
  EXPECT_FALSE(server.SyncCall(&Server::close_handler_called));

  dispatcher.ShutdownAsync();
  dispatcher_shutdown->Wait();
}

TEST(ServerBindingGroup, CloseHandler) {
  fidl_driver_testing::ScopedFakeDriver driver;
  auto [dispatcher, dispatcher_shutdown] = CreateSyncDispatcher();

  struct Server : public fdf::WireServer<test_transport::EmptyProtocol> {};
  Server server;

  auto endpoints = fdf::CreateEndpoints<test_transport::EmptyProtocol>();
  ASSERT_TRUE(endpoints.is_ok()) << endpoints.status_string();
  auto [client_end, server_end] = std::move(*endpoints);

  libsync::Completion got_error;
  std::optional<fidl::UnbindInfo> error;
  int close_handler_count = 0;

  std::optional<fdf::ServerBindingGroup<test_transport::EmptyProtocol>> binding;
  async::PostTask(
      dispatcher.async_dispatcher(),
      [&, &dispatcher = dispatcher, &server_end = server_end, &client_end = client_end] {
        binding.emplace();
        auto add_binding = binding->CreateHandler(
            &server, dispatcher.get(),
            [&error, &got_error, &close_handler_count](fidl::UnbindInfo info) {
              error.emplace(info);
              close_handler_count++;
              got_error.Signal();
            });
        add_binding(std::move(server_end));

        client_end.reset();
      });

  got_error.Wait();
  ASSERT_TRUE(error.has_value());
  EXPECT_TRUE(error->is_peer_closed());
  EXPECT_EQ(1, close_handler_count);

  async::PostTask(dispatcher.async_dispatcher(), [&] { binding.reset(); });
  dispatcher.ShutdownAsync();
  dispatcher_shutdown->Wait();
  EXPECT_EQ(1, close_handler_count);
}

TEST(ServerBindingGroup, CannotBindUnsynchronizedDispatcher) {
  DEBUG_ONLY_TEST_MAY_SKIP();
  fidl_driver_testing::ScopedFakeDriver driver;

  libsync::Completion dispatcher_shutdown;
  zx::result dispatcher = fdf::UnsynchronizedDispatcher::Create(
      {}, "", [&](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown.Signal(); });
  ASSERT_OK(dispatcher.status_value());

  zx::result endpoints = fdf::CreateEndpoints<test_transport::EmptyProtocol>();
  ASSERT_OK(endpoints.status_value());

  struct Server : public fdf::WireServer<test_transport::EmptyProtocol> {};
  Server server;

  std::optional<fdf::ServerBindingGroup<test_transport::EmptyProtocol>> binding;
  libsync::Completion created;
  async::PostTask(dispatcher->async_dispatcher(), [&] {
    binding.emplace();
    auto add_binding =
        binding->CreateHandler(&server, dispatcher->get(), fidl::kIgnoreBindingClosure);
    ASSERT_DEATH(add_binding(std::move(endpoints->server)),
                 "The selected FIDL bindings is thread unsafe\\. A synchronized fdf_dispatcher_t "
                 "is required\\. Ensure the fdf_dispatcher_t does not have the "
                 "\\|FDF_DISPATCHER_OPTION_UNSYNCHRONIZED\\| option\\.");
    binding.reset();
    created.Signal();
  });
  created.Wait();

  dispatcher->ShutdownAsync();
  dispatcher_shutdown.Wait();
}

}  // namespace
