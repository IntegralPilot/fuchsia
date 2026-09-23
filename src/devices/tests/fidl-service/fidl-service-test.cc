// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

namespace {

TEST(FidlServiceTest, ChildBinds) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  loop.StartThread();

  // Create and build the realm.
  auto args = fuchsia_driver_test::RealmArgs();
  args.root_driver("fuchsia-boot:///dtr#meta/test-parent-sys.cm");
  auto realm_builder = component_testing::RealmBuilder::Create();
  driver_test_realm::Setup(realm_builder, loop.dispatcher(), {}, std::move(args));
  auto realm = realm_builder.Build(loop.dispatcher());

  // Start DriverTestRealm.
  zx::result<> boot_result = driver_test_realm::WaitForBootup(realm);
  ASSERT_EQ(ZX_OK, boot_result.status_value());

  // Wait for the child device to bind and appear. The child driver should bind with its string
  // properties. It will then make a call via FIDL and wait for the response before adding the child
  // device.
  auto node = driver_test_realm::WaitForNode(realm, "dev.sys.test.parent.child");
  ASSERT_TRUE(node.is_ok());

  driver_test_realm::ShutdownRealm(realm);
}

}  // namespace
