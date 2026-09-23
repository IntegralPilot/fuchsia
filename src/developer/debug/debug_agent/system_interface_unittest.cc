// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/system_interface.h"

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/mock_system_interface.h"

namespace debug_agent {

namespace {

TEST(SystemInterfaceTest, GetParentJobKoid) {
  auto system_interface = MockSystemInterface::CreateWithData();

  //  j: 1 root
  //    j: 8 job1  /moniker  fuchsia-pkg://devhost/package#meta/component.cm
  //      j: 13 job11
  //      j: 17 job12
  //        j: 18 job121
  //          p: 19 job121-p1
  EXPECT_EQ(17ull, system_interface->GetParentJobKoid(18));
  EXPECT_EQ(8ull, system_interface->GetParentJobKoid(17));
  EXPECT_EQ(8ull, system_interface->GetParentJobKoid(13));
  EXPECT_EQ(1ull, system_interface->GetParentJobKoid(8));
  EXPECT_EQ(ZX_KOID_INVALID, system_interface->GetParentJobKoid(1));
  EXPECT_EQ(ZX_KOID_INVALID, system_interface->GetParentJobKoid(19));
}

TEST(SystemInterfaceTest, GetComponentInfo) {
  auto system_interface = MockSystemInterface::CreateWithData();

  //  j: 1 root
  //    j: 8 job1  /moniker  fuchsia-pkg://devhost/package#meta/component.cm
  //      j: 17 job12
  //        j: 18 job121
  //          p: 19 job121-p1
  auto components = system_interface->GetComponentManager().FindComponentInfo(8);
  ASSERT_FALSE(components.empty());
  ASSERT_EQ(components.size(), 1ull);
  auto component_info = components[0];
  EXPECT_EQ("/moniker", component_info.moniker);
  EXPECT_EQ("fuchsia-pkg://devhost/package#meta/component.cm", component_info.url);

  components =
      system_interface->GetComponentManager().FindComponentInfo(*system_interface->GetProcess(19));
  ASSERT_FALSE(components.empty());
  ASSERT_EQ(components.size(), 1ull);
  component_info = components[0];
  EXPECT_EQ("/moniker", component_info.moniker);
  EXPECT_EQ("fuchsia-pkg://devhost/package#meta/component.cm", component_info.url);
}

TEST(SystemInterfaceTest, GetMultipleComponentInfo) {
  auto system_interface = MockSystemInterface::CreateWithData();

  //  j: 1 root
  //    j: 28 job3 multiple-components
  //      p: 29 job2-p1 process-host
  //        t: 30 initial-thread
  //        t: 31 second-thread

  // Note: the order of components returned here is not guaranteed.
  auto components = system_interface->GetComponentManager().FindComponentInfo(28);
  ASSERT_FALSE(components.empty());
  ASSERT_EQ(components.size(), 4ull);
  bool found = false;
  found = std::any_of(components.cbegin(), components.cend(), [](const auto& component) {
    return component.moniker == "a/generated/moniker:1000" &&
           component.url == "fuchsia-boot:///url#meta/cm0.base.cm";
  });
  EXPECT_TRUE(found);
  found = false;

  found = std::any_of(components.cbegin(), components.cend(), [](const auto& component) {
    return component.moniker == "a/generated/moniker:1001" &&
           component.url == "fuchsia-boot:///url#meta/cm1.cm";
  });
  EXPECT_TRUE(found);
  found = false;

  found = std::any_of(components.cbegin(), components.cend(), [](const auto& component) {
    return component.moniker == "bootstrap/hosts:host-1" &&
           component.url == "fuchsia-boot:///url#meta/host.cm";
  });
  EXPECT_TRUE(found);
}

}  // namespace

}  // namespace debug_agent
