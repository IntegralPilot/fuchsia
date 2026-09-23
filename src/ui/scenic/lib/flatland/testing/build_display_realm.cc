// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/testing/build_display_realm.h"

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>

namespace flatland::testing {

component_testing::RealmRoot BuildFakeDisplayRealm(async_dispatcher_t* dispatcher,
                                                   const DisplayRealmConfig& config) {
  component_testing::RealmBuilder builder = component_testing::RealmBuilder::Create();
  static const std::string kFakeDisplayStackHostChildName = "fake-display-stack-host";
  builder.AddChild(std::string(kFakeDisplayStackHostChildName), "#meta/fake-display-stack-host.cm");

  // Route capabilities from the test to the child component.
  builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Protocol{.name = "fuchsia.sysmem2.Allocator"},
                       component_testing::Protocol{.name = "fuchsia.tracing.provider.Registry"}},
      .source = component_testing::ParentRef(),
      .targets = {component_testing::ChildRef{.name = kFakeDisplayStackHostChildName}},
  });

  // Route capabilities from the child component to the test.
  builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Service{.name = "fuchsia.hardware.display.Service"}},
      .source = component_testing::ChildRef{.name = kFakeDisplayStackHostChildName},
      .targets = {component_testing::ParentRef()},
  });

  // Route configurations to fake-display-stack-host.
  builder.InitMutableConfigFromPackage(kFakeDisplayStackHostChildName);
  if (config.active_width_px != 0) {
    builder.SetConfigValue(kFakeDisplayStackHostChildName, "active_width_px",
                           component_testing::ConfigValue::Uint32(config.active_width_px));
  }
  if (config.active_height_px != 0) {
    builder.SetConfigValue(kFakeDisplayStackHostChildName, "active_height_px",
                           component_testing::ConfigValue::Uint32(config.active_height_px));
  }
  if (config.refresh_rate_millihertz != 0) {
    builder.SetConfigValue(kFakeDisplayStackHostChildName, "refresh_rate_millihertz",
                           component_testing::ConfigValue::Uint32(config.refresh_rate_millihertz));
  }
  if (config.max_layer_count != 0) {
    builder.SetConfigValue(kFakeDisplayStackHostChildName, "max_layer_count",
                           component_testing::ConfigValue::Uint32(config.max_layer_count));
  }
  return builder.Build(dispatcher);
}

}  // namespace flatland::testing
