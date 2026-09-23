// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.transport.test/cpp/wire.h>
#include <fuchsia/driver/test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fidl/cpp/synchronous_interface_ptr.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>

#include <fbl/unique_fd.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

using fuchsia_driver_transport_test::TestDevice;
using fuchsia_driver_transport_test::TestDeviceChild;

class RuntimeTest : public gtest::TestLoopFixture {
 protected:
  void SetUp() override {
    loop_.StartThread();
    // Create and build the realm.
    auto realm_builder = component_testing::RealmBuilder::Create();
    driver_test_realm::Setup(realm_builder, loop_.dispatcher(), {}, {});
    realm_ =
        std::make_unique<component_testing::RealmRoot>(realm_builder.Build(loop_.dispatcher()));

    // Start DriverTestRealm.
    zx::result<> boot_result = driver_test_realm::WaitForBootup(*realm_);
    ASSERT_EQ(ZX_OK, boot_result.status_value());

    auto node = driver_test_realm::WaitForNode(*realm_, "dev.sys.test.parent");
    ASSERT_TRUE(node.is_ok());
    node = driver_test_realm::WaitForNode(*realm_, "dev.sys.test.parent.child");
    ASSERT_TRUE(node.is_ok());

    // TODO(https://fxbug.dev/377735979): Connect using a different mechanism.
    fbl::unique_fd fd;
    auto exposed = realm_->component().CloneExposedDir();
    ASSERT_EQ(fdio_fd_create(exposed.TakeChannel().release(), fd.reset_and_get_address()), ZX_OK);
    // Wait for parent device.
    zx::result parent_channel =
        device_watcher::RecursiveWaitForFile(fd.get(), "dev-topological/sys/test/parent");
    ASSERT_EQ(parent_channel.status_value(), ZX_OK);
    parent_client = fidl::ClientEnd<TestDevice>(std::move(parent_channel.value()));
    ASSERT_TRUE(parent_client.is_valid());
    // Wait for child device.
    zx::result child_channel =
        device_watcher::RecursiveWaitForFile(fd.get(), "dev-topological/sys/test/parent/child");
    ASSERT_EQ(child_channel.status_value(), ZX_OK);
    child_client = fidl::ClientEnd<TestDeviceChild>(std::move(child_channel.value()));
    ASSERT_TRUE(child_client.is_valid());
  }

  void TearDown() override { driver_test_realm::ShutdownRealm(*realm_); }

  // Sets test data in the parent device that can be retrieved by the child device.
  void ParentSetTestData(uint8_t* data_to_send, size_t size);
  // Sends a FIDL request to the child device to retrieve data from the parent device
  // using its runtime channel.
  // Asserts that the data matches |want_data| and |want_size|.
  void GetParentDataOverDriverTransport(const void* want_data, size_t want_size);

  fidl::ClientEnd<TestDeviceChild> child_client;
  fidl::ClientEnd<TestDevice> parent_client;

 private:
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::unique_ptr<component_testing::RealmRoot> realm_;
};

void RuntimeTest::ParentSetTestData(uint8_t* data_to_send, size_t size) {
  auto data = fidl::VectorView<uint8_t>::FromExternal(data_to_send, size);

  auto response = fidl::WireCall(parent_client)->SetTestData(std::move(data));
  ASSERT_EQ(ZX_OK, response.status());
  zx_status_t call_status = ZX_OK;
  if (response->is_error()) {
    call_status = response->error_value();
  }
  ASSERT_EQ(call_status, ZX_OK);
}

void RuntimeTest::GetParentDataOverDriverTransport(const void* want_data, size_t want_size) {
  auto response = fidl::WireCall(child_client)->GetParentDataOverDriverTransport();
  ASSERT_EQ(ZX_OK, response.status());

  zx_status_t call_status = ZX_OK;
  if (response->is_error()) {
    call_status = response->error_value();
  }
  ASSERT_EQ(call_status, ZX_OK);

  auto data = response->value()->out;
  ASSERT_EQ(data.size(), want_size);
  ASSERT_EQ(memcmp(data.data(), want_data, want_size), 0);
}

TEST_F(RuntimeTest, TransferOverDriverTransport) {
  uint8_t kTestString[] = "some test string";
  ASSERT_NO_FATAL_FAILURE(ParentSetTestData(kTestString, std::size(kTestString)));
  ASSERT_NO_FATAL_FAILURE(GetParentDataOverDriverTransport(kTestString, std::size(kTestString)));

  uint8_t kTestString2[] = "another test string";
  ASSERT_NO_FATAL_FAILURE(ParentSetTestData(kTestString2, std::size(kTestString)));
  ASSERT_NO_FATAL_FAILURE(GetParentDataOverDriverTransport(kTestString2, std::size(kTestString)));
}
