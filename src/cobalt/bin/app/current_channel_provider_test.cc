// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/cobalt/bin/app/current_channel_provider.h"

#include <fidl/fuchsia.update.channel/cpp/fidl.h>
#include <fidl/fuchsia.update.channel/cpp/test_base.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>

#include <memory>
#include <string>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace cobalt {
namespace {

using ::inspect::testing::ChildrenMatch;
using ::inspect::testing::NameMatches;
using ::inspect::testing::NodeMatches;
using ::inspect::testing::PropertyList;
using ::inspect::testing::StringIs;
using ::testing::UnorderedElementsAre;

class FakeChannelProvider : public fidl::testing::TestBase<fuchsia_update_channel::Provider> {
 public:
  explicit FakeChannelProvider(std::string channel) : channel_(std::move(channel)) {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FX_NOTIMPLEMENTED() << name << " is not implemented";
  }

  void GetCurrent(GetCurrentCompleter::Sync& completer) override {
    completer.Reply(fidl::Response<fuchsia_update_channel::Provider::GetCurrent>(channel_));
  }

 private:
  std::string channel_;
};

class ChannelProviderClosesConnection
    : public fidl::testing::TestBase<fuchsia_update_channel::Provider> {
 public:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FX_NOTIMPLEMENTED() << name << " is not implemented";
  }

  void GetCurrent(GetCurrentCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_PEER_CLOSED);
  }
};

class CurrentChannelProviderTest : public gtest::TestLoopFixture {
 protected:
  zx::result<CurrentChannelProvider> SetUpConnections(
      const std::string& current_channel,
      std::unique_ptr<fidl::Server<fuchsia_update_channel::Provider>> server) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_update_channel::Provider>();
    if (!endpoints.is_ok()) {
      return zx::error(ZX_ERR_NOT_CONNECTED);
    }

    server_ = std::move(server);
    fidl::BindServer(dispatcher(), std::move(endpoints->server), server_.get());
    return zx::ok(CurrentChannelProvider(dispatcher(), std::move(endpoints->client),
                                         inspector_.GetRoot().CreateChild("system_data"),
                                         current_channel));
  }

  inspect::Hierarchy InspectTree() {
    fpromise::result<inspect::Hierarchy> result = inspect::ReadFromVmo(inspector_.DuplicateVmo());
    FX_CHECK(result.is_ok());
    return result.take_value();
  }

 private:
  inspect::Inspector inspector_;
  std::unique_ptr<fidl::Server<fuchsia_update_channel::Provider>> server_;
};

TEST_F(CurrentChannelProviderTest, GetCurrentChannel) {
  auto channel_provider_server = std::make_unique<FakeChannelProvider>("test_channel");
  zx::result<CurrentChannelProvider> channel_provider =
      SetUpConnections("<unset>", std::move(channel_provider_server));
  ASSERT_TRUE(channel_provider.is_ok());

  std::string channel;
  channel_provider->GetCurrentChannel(
      [&channel](const std::string& current_channel) { channel = current_channel; });

  RunLoopUntilIdle();
  EXPECT_EQ(channel, "test_channel");
}

TEST_F(CurrentChannelProviderTest, GetCurrentChannelFails) {
  auto channel_provider_server = std::make_unique<ChannelProviderClosesConnection>();
  zx::result<CurrentChannelProvider> channel_provider =
      SetUpConnections("<unset>", std::move(channel_provider_server));
  ASSERT_TRUE(channel_provider.is_ok());

  std::string channel;
  channel_provider->GetCurrentChannel(
      [&channel](const std::string& current_channel) { channel = current_channel; });

  RunLoopUntilIdle();
  EXPECT_EQ(channel, "");
}

TEST_F(CurrentChannelProviderTest, SetsInspectChannel) {
  auto channel_provider_server = std::make_unique<FakeChannelProvider>("test_channel");
  zx::result<CurrentChannelProvider> channel_provider =
      SetUpConnections("<unset>", std::move(channel_provider_server));
  ASSERT_TRUE(channel_provider.is_ok());

  EXPECT_THAT(InspectTree(),
              AllOf(NodeMatches(NameMatches("root")),
                    ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                        NameMatches("system_data"),
                        PropertyList(UnorderedElementsAre(StringIs("channel", "<unset>")))))))));

  std::string channel;
  channel_provider->GetCurrentChannel(
      [&channel](const std::string& current_channel) { channel = current_channel; });

  RunLoopUntilIdle();

  EXPECT_THAT(
      InspectTree(),
      AllOf(NodeMatches(NameMatches("root")),
            ChildrenMatch(UnorderedElementsAre(NodeMatches(
                AllOf(NameMatches("system_data"),
                      PropertyList(UnorderedElementsAre(StringIs("channel", "test_channel")))))))));
}

}  // namespace
}  // namespace cobalt
