// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.suspend/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/test.sagcontrol/cpp/fidl.h>
#include <fidl/test.suspendcontrol/cpp/fidl.h>
#include <lib/fidl/cpp/channel.h>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/test_loop_fixture.h>

#include "src/power/testing/client/cpp/power_framework_test_realm.h"

class PowerFrameworkTestRealmTest : public gtest::TestLoopFixture {};

TEST_F(PowerFrameworkTestRealmTest, ConnectTest) {
  auto realm_builder = component_testing::RealmBuilder::Create();
  power_framework_test_realm::Setup(realm_builder);
  auto realm = realm_builder.Build(dispatcher());

  std::vector<fuchsia_power_broker::PowerLevel> levels = {0, 1};
  std::vector<fuchsia_power_broker::LevelDependency> level_deps{};

  zx::result<fidl::Endpoints<fuchsia_power_broker::ElementRunner>> element_runner =
      fidl::CreateEndpoints<fuchsia_power_broker::ElementRunner>();

  fuchsia_power_broker::ElementSchema schema{{
      .element_name = "power-framework-test-realm-test-element",
      .initial_current_level = static_cast<uint8_t>(0),
      .valid_levels = std::move(levels),
      .dependencies = std::move(level_deps),
      .element_runner = std::move(element_runner->client),
  }};

  auto add_element_result = fidl::Call(*realm.component().Connect<fuchsia_power_broker::Topology>())
                                ->AddElement(std::move(schema));
  ASSERT_EQ(true, add_element_result.is_ok()) << add_element_result.error_value();

  auto get_power_elements_result =
      fidl::Call(*realm.component().Connect<fuchsia_power_system::ActivityGovernor>())
          ->GetPowerElements();
  ASSERT_EQ(true, get_power_elements_result.is_ok()) << get_power_elements_result.error_value();

  auto watch_result =
      fidl::Call(*realm.component().Connect<fuchsia_power_suspend::Stats>())->Watch();
  ASSERT_EQ(true, watch_result.is_ok()) << watch_result.error_value();

  auto get_result = fidl::Call(*realm.component().Connect<test_sagcontrol::State>())->Get();
  ASSERT_EQ(true, get_result.is_ok()) << get_result.error_value();

  auto set_result = fidl::Call(*realm.component().Connect<test_suspendcontrol::Device>())
                        ->SetSuspendStates({{{{
                            fuchsia_hardware_power_suspend::SuspendState{{.resume_latency = 100}},
                        }}}});
  ASSERT_EQ(true, set_result.is_ok()) << set_result.error_value();

  auto power_elements_result =
      fidl::Call(*realm.component().Connect<fuchsia_power_system::ActivityGovernor>())
          ->GetPowerElements();
  ASSERT_EQ(true, power_elements_result.is_ok()) << power_elements_result.error_value();

  auto suspend_states =
      fidl::Call(*power_framework_test_realm::ConnectToSuspender(realm))->GetSuspendStates();
  ASSERT_EQ(true, suspend_states.is_ok()) << suspend_states.error_value();
}
