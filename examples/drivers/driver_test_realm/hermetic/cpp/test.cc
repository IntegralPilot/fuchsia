// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.hardware.sample/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/synchronous_interface_ptr.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>

#include <fbl/unique_fd.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

// [START example]
class DriverTestRealmTest : public gtest::TestLoopFixture {};

TEST_F(DriverTestRealmTest, DriversExist) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  loop.StartThread("bg");

  // Create and build the realm.
  auto realm_builder = component_testing::RealmBuilder::Create();
  driver_test_realm::Setup(realm_builder, loop.dispatcher(), driver_test_realm::Options{},
                           fuchsia_driver_test::RealmArgs{});
  auto realm = realm_builder.Build(loop.dispatcher());
  auto boot_result = driver_test_realm::WaitForBootup(realm);
  ASSERT_EQ(ZX_OK, boot_result.status_value());

  fbl::unique_fd fd;
  auto exposed = realm.component().CloneExposedDir();
  ASSERT_EQ(ZX_OK, fdio_fd_create(exposed.TakeChannel().release(), fd.reset_and_get_address()));

  // Wait for driver.
  auto node = driver_test_realm::WaitForNode(realm, "dev.sys.test.sample_driver");
  ASSERT_TRUE(node.is_ok());

  // TODO(https://fxbug.dev/377735979): Connect using a different mechanism.
  zx::result channel =
      device_watcher::RecursiveWaitForFile(fd.get(), "dev-topological/sys/test/sample_driver");
  ASSERT_EQ(channel.status_value(), ZX_OK);

  fidl::ClientEnd<fuchsia_hardware_sample::Echo> client(std::move(*channel));

  // Send a FIDL request.
  constexpr std::string_view sent_string = "hello";
  fidl::WireResult result =
      fidl::WireCall(client)->EchoString(fidl::StringView::FromExternal(sent_string));
  ASSERT_EQ(ZX_OK, result.status());
  ASSERT_EQ(sent_string, result.value().response.get());

  driver_test_realm::ShutdownRealm(realm);
}
// [END example]
