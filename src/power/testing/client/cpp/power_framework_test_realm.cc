// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/power/testing/client/cpp/power_framework_test_realm.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>

namespace power_framework_test_realm {

constexpr const char kComponentName[] = "power_framework_test_realm";
constexpr const char kPowerFrameworkTestRealmUrl[] = "power-framework#meta/power-framework.cm";

void Setup(component_testing::RealmBuilder& realm_builder) {
  realm_builder.AddChild(kComponentName, kPowerFrameworkTestRealmUrl)
      .AddRoute(component_testing::Route{
          .capabilities =
              {
                  component_testing::Protocol{"fuchsia.power.broker.Topology"},
                  component_testing::Protocol{"fuchsia.power.suspend.Stats"},
                  component_testing::Protocol{"fuchsia.power.system.ActivityGovernor"},
                  component_testing::Protocol{"test.sagcontrol.State"},
                  component_testing::Protocol{"test.suspendcontrol.Device"},
                  component_testing::Service{"fuchsia.hardware.power.suspend.SuspendService"},
              },
          .source = component_testing::ChildRef{kComponentName},
          .targets = {component_testing::ParentRef()}});
}

zx::result<fidl::ClientEnd<fuchsia_hardware_power_suspend::Suspender>> ConnectToSuspender(
    component_testing::RealmRoot& root) {
  fidl::UnownedClientEnd<fuchsia_io::Directory> svc(
      root.component().exposed().unowned_channel()->get());
  return component::ConnectAtMember<fuchsia_hardware_power_suspend::SuspendService::Suspender>(svc);
}

}  // namespace power_framework_test_realm
