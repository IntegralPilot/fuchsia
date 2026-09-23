// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.service.test/cpp/fidl.h>
#include <fidl/fidl.service.test/cpp/wire_test_base.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/component/tests/echo_server.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fidl/cpp/string.h>
#include <lib/fit/function.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace {

using namespace component_testing;

class LocalEchoServer : public LocalCppComponent {
 public:
  static constexpr const char kAlternateServiceInstance[] = "alternate";

  explicit LocalEchoServer(async_dispatcher_t* dispatcher, unsigned int* called)
      : dispatcher_(dispatcher), called_(called) {}

  void OnStart() override {
    servers_.emplace("default", dispatcher_, called_);
    ASSERT_EQ(outgoing()
                  ->AddUnmanagedProtocol<fidl_service_test::Echo>(servers_.back().CreateHandler())
                  .status_value(),
              ZX_OK);
    {
      servers_.emplace("default", dispatcher_, called_);
      auto result = outgoing()->AddService<fidl_service_test::EchoService>(
          fidl_service_test::EchoService::InstanceHandler({
              .foo = servers_.back().CreateHandler(),
              .bar = servers_.back().CreateHandler(),
          }));
      ASSERT_EQ(result.status_value(), ZX_OK);
    }
    // Add another service instance.  This simulates how an aggregated service would present
    // multiple service instances to a client.
    {
      servers_.emplace(kAlternateServiceInstance, dispatcher_, called_);
      auto result = outgoing()->AddService<fidl_service_test::EchoService>(
          fidl_service_test::EchoService::InstanceHandler({
              .foo = servers_.back().CreateHandler(),
              .bar = servers_.back().CreateHandler(),
          }),
          kAlternateServiceInstance);
      ASSERT_EQ(result.status_value(), ZX_OK);
    }
  }

 private:
  async_dispatcher_t* dispatcher_;
  unsigned int* called_;
  std::queue<EchoCommon> servers_;
};

class IncomingTest : public gtest::RealLoopFixture {};

TEST_F(IncomingTest, ConnectsToProtocolInNamespace) {
  auto realm_builder = RealmBuilder::Create();

  realm_builder.AddChild("echo_client", "#meta/echo_client.cm",
                         ChildOptions{.startup_mode = StartupMode::EAGER});
  unsigned int called = 0;
  realm_builder.AddLocalChild("echo_server", [dispatcher = dispatcher(), called_ptr = &called]() {
    return std::make_unique<LocalEchoServer>(dispatcher, called_ptr);
  });
  realm_builder.AddRoute(Route{
      .capabilities = {Protocol{fidl::DiscoverableProtocolName<fidl_service_test::Echo>}},
      .source = ChildRef{"echo_server"},
      .targets = {ChildRef{"echo_client"}},
  });

  auto realm = realm_builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });

  RunLoopUntil([&called]() { return called > 0; });
}

TEST_F(IncomingTest, ConnectsToServiceInNamespace) {
  auto realm_builder = RealmBuilder::Create();

  realm_builder.AddChild("echo_client", "#meta/echo_service_client.cm",
                         ChildOptions{.startup_mode = StartupMode::EAGER});
  unsigned int called = 0;
  realm_builder.AddLocalChild("echo_server", [dispatcher = dispatcher(), called_ptr = &called]() {
    return std::make_unique<LocalEchoServer>(dispatcher, called_ptr);
  });
  realm_builder.AddRoute(Route{
      .capabilities = {Service{fidl_service_test::EchoService::Name}},
      .source = ChildRef{"echo_server"},
      .targets = {ChildRef{"echo_client"}},
  });

  auto realm = realm_builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });

  RunLoopUntil([&called]() { return called > 0; });
}

TEST_F(IncomingTest, ConnectsToAggregatedServiceInNamespace) {
  auto realm_builder = RealmBuilder::Create();

  realm_builder.AddChild("echo_client", "#meta/echo_service_watcher_client.cm",
                         ChildOptions{.startup_mode = StartupMode::EAGER});

  unsigned int called = 0;
  realm_builder.AddLocalChild("echo_server", [dispatcher = dispatcher(), called_ptr = &called]() {
    return std::make_unique<LocalEchoServer>(dispatcher, called_ptr);
  });
  realm_builder.AddRoute(Route{
      .capabilities = {Service{fidl_service_test::EchoService::Name}},
      .source = ChildRef{"echo_server"},
      .targets = {ChildRef{"echo_client"}},
  });

  auto realm = realm_builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });
  // Here we expect 4 calls to the servers:
  // The client component runs a watcher with the default constructor:
  //  sees instance "default" and calls EchoString (1)
  //  sees instance "alternate" and calls EchoString (2)
  // Then the client gets the idle callback and stops.
  // The the client does runs again with a manually set service root, making 2 more calls.
  RunLoopUntil([&called]() { return called >= 4; });
}

}  // namespace
