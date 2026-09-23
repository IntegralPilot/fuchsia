// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START import_statement_cpp]
#include <lib/sys/component/cpp/testing/realm_builder.h>
// [END import_statement_cpp]

#include <fidl/fidl.examples.routing.echo/cpp/fidl.h>
#include <fuchsia/component/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fidl/cpp/string.h>
#include <zircon/status.h>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

// [START use_namespace_cpp]
// NOLINTNEXTLINE
using namespace component_testing;
// [END use_namespace_cpp]

class RealmBuilderTest : public ::gtest::RealLoopFixture {};

// This test demonstrates constructing a realm with two child components
// and verifying the `fidl.examples.routing.Echo` protocol.
TEST_F(RealmBuilderTest, RoutesFromEcho) {
  // [START init_realm_builder_cpp]
  RealmBuilder builder = RealmBuilder::Create();
  // [END init_realm_builder_cpp]

  // [START add_component_cpp]
  // [START add_server_cpp]
  // Add component server to the realm, which is fetched using a URL.
  builder.AddChild("echo_server",
                   "fuchsia-pkg://fuchsia.com/realm-builder-examples#meta/echo_server.cm");
  // [END add_server_cpp]
  // Add component to the realm, which is fetched using a fragment-only URL. The
  // child is not exposing a service, so the `EAGER` option ensures the child
  // starts when the realm is built.
  builder.AddChild("echo_client", "#meta/echo_client.cm",
                   ChildOptions{.startup_mode = StartupMode::EAGER});
  // [END add_component_cpp]

  // [START route_between_children_cpp]
  builder.AddRoute(Route{.capabilities = {Protocol{.name = "fidl.examples.routing.echo.Echo"}},
                         .source = ChildRef{"echo_server"},
                         .targets = {ChildRef{"echo_client"}}});
  // [END route_between_children_cpp]

  // [START route_to_test_cpp]
  builder.AddRoute(Route{.capabilities = {Protocol{.name = "fidl.examples.routing.echo.Echo"}},
                         .source = ChildRef{"echo_server"},
                         .targets = {ParentRef()}});
  // [END route_to_test_cpp]

  // [START route_from_test_cpp]
  builder.AddRoute(Route{.capabilities = {Protocol{.name = "fuchsia.logger.LogSink"}},
                         .source = ParentRef(),
                         .targets = {ChildRef{"echo_server"}, ChildRef{"echo_client"}}});
  // [END route_from_test_cpp]

  // [START build_realm_cpp]
  RealmRoot realm = builder.Build(dispatcher());
  // [END build_realm_cpp]

  // [START get_child_name_cpp]
  std::cout << "Child Name: " << realm.component().GetChildName() << '\n';
  // [END get_child_name_cpp]

  // [START call_echo_cpp]
  zx::result<fidl::ClientEnd<fidl_examples_routing_echo::Echo>> client_end =
      realm.component().Connect<fidl_examples_routing_echo::Echo>();
  ASSERT_FALSE(client_end.is_error()) << client_end.status_string();
  fidl::SyncClient echo_client(std::move(*client_end));

  fidl::Result<::fidl_examples_routing_echo::Echo::EchoString> result =
      echo_client->EchoString({"hello"});
  ASSERT_EQ(result->response(), "hello");
  // [END call_echo_cpp]
}

// [START mock_component_impl_cpp]
class LocalEchoServerImpl : public fidl::Server<fidl_examples_routing_echo::Echo>,
                            public LocalCppComponent {
 public:
  explicit LocalEchoServerImpl(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  // Override `OnStart` from `LocalComponentImpl` class.
  void OnStart() override {
    // When `OnStart()` is called, this implementation can call methods to
    // access handles to the component's incoming capabilities (`ns()` and
    // `svc()`) and outgoing capabilities (`outgoing()`).;
    zx::result<> result = outgoing()->AddUnmanagedProtocol<fidl_examples_routing_echo::Echo>(
        bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
    if (result.is_error()) {
      ZX_PANIC("%s", result.status_string());
    }
  }

  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    completer.Reply({{.response = request.value()}});
  }

 private:
  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fidl_examples_routing_echo::Echo> bindings_;
};
// [END mock_component_impl_cpp]

// This test demonstrates constructing a realm with a mocked LocalComponent
// implementation of the `fidl.examples.routing.Echo` protocol.
TEST_F(RealmBuilderTest, RoutesFromMockEcho) {
  RealmBuilder builder = RealmBuilder::Create();

  // [START add_mock_component_cpp]
  // Add component to the realm, providing a mock implementation
  builder.AddLocalChild("echo_server",
                        [&]() { return std::make_unique<LocalEchoServerImpl>(dispatcher()); });
  // [END add_mock_component_cpp]

  builder.AddRoute(Route{.capabilities = {Protocol{.name = "fuchsia.logger.LogSink"}},
                         .source = ParentRef(),
                         .targets = {ChildRef{"echo_server"}}});

  builder.AddRoute(Route{.capabilities = {Protocol{.name = "fidl.examples.routing.echo.Echo"}},
                         .source = ChildRef{"echo_server"},
                         .targets = {ParentRef()}});

  RealmRoot realm = builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });

  zx::result<fidl::ClientEnd<fidl_examples_routing_echo::Echo>> client_end =
      realm.component().Connect<fidl_examples_routing_echo::Echo>();

  ASSERT_FALSE(client_end.is_error()) << client_end.status_string();

  fidl::Client echo_client(std::move(*client_end), dispatcher());

  bool was_called = false;
  echo_client->EchoString({{.value = "hello"}})
      .ThenExactlyOnce([&](fidl::Result<fidl_examples_routing_echo::Echo::EchoString>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
        auto response = *result->response();
        was_called = true;
        ASSERT_EQ(response, "hello");
      });
  RunLoopUntil([&]() { return was_called; });
}
