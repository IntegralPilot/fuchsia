// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.test.drivers.power/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/syslog/cpp/macros.h>

#include <unordered_set>

#include <fbl/unique_fd.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace power_integration_test {

class PowerIntegrationTest : public gtest::TestLoopFixture {};

TEST_F(PowerIntegrationTest, MetadataPassing) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  loop.StartThread();

  auto builder = component_testing::RealmBuilder::Create();
  builder.AddChild(
      "power-broker", "#meta/mock-power-broker.cm",
      component_testing::ChildOptions{.startup_mode = component_testing::StartupMode::LAZY});

  builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Protocol{
          .name = "fuchsia.test.drivers.power.GetPowerElements"}},
      .source = {component_testing::ChildRef{"power-broker"}},
      .targets = {component_testing::ParentRef{}},
  });

  std::vector<fuchsia_component_test::Capability> offers = {
      fuchsia_component_test::Capability::WithProtocol({{
          .name = "fuchsia.power.broker.Topology",
      }}),
  };

  auto realm_args = fuchsia_driver_test::RealmArgs();
  realm_args.root_driver() = "fuchsia-boot:///#meta/platform-bus.cm";

  driver_test_realm::OptionsBuilder options;
  options.driver_offers(component_testing::ChildRef{"power-broker"}, offers);

  driver_test_realm::Setup(builder, loop.dispatcher(), options.Build(), std::move(realm_args));

  auto test_realm = builder.Build(loop.dispatcher());
  auto boot_result = driver_test_realm::WaitForBootup(test_realm);
  ASSERT_EQ(ZX_OK, boot_result.status_value());

  auto get_elements_result =
      test_realm.component().Connect<fuchsia_test_drivers_power::GetPowerElements>(
          "fuchsia.test.drivers.power.GetPowerElements");
  ASSERT_TRUE(get_elements_result.is_ok());
  fidl::WireSyncClient<fuchsia_test_drivers_power::GetPowerElements> elements_client(
      std::move(get_elements_result.value()));

  std::unordered_set<std::string> expected_elements{"pe-fake-parent", "pe-fake-child"};

  while (expected_elements.size() > 0) {
    auto resp = elements_client->GetElements();
    fidl::VectorView<fidl::StringView> added_elements = resp->elements;
    for (fidl::StringView e : added_elements) {
      std::string element_name(e.data(), e.size());
      ASSERT_NE(expected_elements.end(), expected_elements.find(element_name));
      expected_elements.erase(element_name);
      FX_LOGS(INFO) << "Found element named '" << element_name << "'";
    }
  }

  driver_test_realm::ShutdownRealm(test_realm);
}
}  // namespace power_integration_test
