// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/directory.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>

#include <zxtest/zxtest.h>

TEST(DriverTransportTest, ParentChildExists) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  loop.StartThread();

  // Create and build the realm.
  auto realm_builder = component_testing::RealmBuilder::Create();
  driver_test_realm::Setup(realm_builder, loop.dispatcher(), {}, {});
  auto realm = realm_builder.Build(loop.dispatcher());
  auto boot_result = driver_test_realm::WaitForBootup(realm);
  ASSERT_EQ(ZX_OK, boot_result.status_value());

  {
    // Wait for parent driver.
    auto node = driver_test_realm::WaitForNode(realm, "dev.sys.test.transport-child");
    ASSERT_TRUE(node.is_ok());
  }

  {
    // Wait for child driver.
    auto node = driver_test_realm::WaitForNode(realm, "dev.sys.test.transport-child.test");
    ASSERT_TRUE(node.is_ok());
  }

  driver_test_realm::ShutdownRealm(realm);
}
