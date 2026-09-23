// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device.test/cpp/wire.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fuchsia/driver/development/cpp/fidl.h>
#include <fuchsia/driver/test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/ddk/driver.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/directory.h>
#include <lib/sys/cpp/component_context.h>

#include <bind/bindlib/codegen/testlib/cpp/bind.h>
#include <bind/bindlibparent/codegen/testlib/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

namespace lib = bind_bindlib_codegen_testlib;
namespace parent = bind_bindlibparent_codegen_testlib;

const std::string kChildDevicePath = "dev.sys.test.parent";

class BindLibToFidlCodeGenTest : public testing::Test {
 protected:
  void SetUp() override {
    loop_.StartThread();
    // Create and build the realm.
    realm_builder_ = component_testing::RealmBuilder::Create();
    driver_test_realm::Setup(*realm_builder_, loop_.dispatcher(), {}, {});
    realm_ = realm_builder_->Build(loop_.dispatcher());

    // Start DriverTestRealm.
    zx::result<> boot_result = driver_test_realm::WaitForBootup(*realm_);
    ASSERT_EQ(ZX_OK, boot_result.status_value());

    fbl::unique_fd fd;
    auto exposed = realm_->component().CloneExposedDir();
    ASSERT_EQ(fdio_fd_create(exposed.TakeChannel().release(), fd.reset_and_get_address()), ZX_OK);

    // Wait for the child device to bind and appear. The child device should bind with its string
    // properties.
    auto node = driver_test_realm::WaitForNode(*realm_, "dev.sys.test.parent.child");
    ASSERT_TRUE(node.is_ok());

    // Connect to the DriverDevelopment service.
    zx_status_t status = realm_->component().Connect(driver_dev_.NewRequest());
    ASSERT_EQ(status, ZX_OK);
  }

  void TearDown() override { driver_test_realm::ShutdownRealm(*realm_); }

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::optional<component_testing::RealmBuilder> realm_builder_;
  std::optional<component_testing::RealmRoot> realm_;
  fuchsia::driver::development::ManagerSyncPtr driver_dev_;
};

TEST_F(BindLibToFidlCodeGenTest, DeviceProperties) {
  fuchsia::driver::development::NodeInfoIteratorSyncPtr iterator;
  ASSERT_EQ(ZX_OK, driver_dev_->GetNodeInfo({kChildDevicePath}, iterator.NewRequest(),
                                            /* exact_match= */ true));

  std::vector<fuchsia::driver::development::NodeInfo> devices;
  ASSERT_EQ(iterator->GetNext(&devices), ZX_OK);

  ASSERT_EQ(devices.size(), 1lu);
  auto& props = devices[0].node_property_list();
  ASSERT_EQ(static_cast<size_t>(9), props.size());

  ASSERT_EQ(bind_fuchsia::PROTOCOL, props[0].key.string_value());
  ASSERT_TRUE(props[0].value.is_int_value());
  ASSERT_EQ(3u, props[0].value.int_value());

  ASSERT_EQ(bind_fuchsia::PCI_VID, props[1].key.string_value());
  ASSERT_TRUE(props[1].value.is_int_value());
  ASSERT_EQ(lib::BIND_PCI_VID_PIE, props[1].value.int_value());

  ASSERT_EQ(bind_fuchsia::PCI_DID, props[2].key.string_value());
  ASSERT_TRUE(props[2].value.is_int_value());
  ASSERT_EQ(1234u, props[2].value.int_value());

  ASSERT_EQ("bindlib.codegen.testlib.kinglet", props[3].key.string_value());
  ASSERT_EQ(lib::KINGLET, props[3].key.string_value());
  ASSERT_TRUE(props[3].value.is_string_value());
  ASSERT_EQ("firecrest", props[3].value.string_value());

  ASSERT_EQ("bindlib.codegen.testlib.Moon", props[4].key.string_value());
  ASSERT_EQ(lib::MOON, props[4].key.string_value());
  ASSERT_TRUE(props[4].value.is_string_value());
  ASSERT_EQ("bindlib.codegen.testlib.Moon.Half", props[4].value.string_value());
  ASSERT_EQ(lib::MOON_HALF, props[4].value.string_value());

  ASSERT_EQ("bindlib.codegen.testlib.bobolink", props[5].key.string_value());
  ASSERT_EQ(lib::BOBOLINK, props[5].key.string_value());
  ASSERT_TRUE(props[5].value.is_int_value());
  ASSERT_EQ(10u, props[5].value.int_value());

  ASSERT_EQ("bindlib.codegen.testlib.flag", props[6].key.string_value());
  ASSERT_EQ(lib::FLAG, props[6].key.string_value());
  ASSERT_TRUE(props[6].value.is_bool_value());
  ASSERT_TRUE(props[6].value.bool_value());
  ASSERT_EQ(lib::FLAG_ENABLE, props[6].value.bool_value());

  ASSERT_EQ("bindlibparent.codegen.testlib.Pizza", props[7].key.string_value());
  ASSERT_EQ(parent::PIZZA, props[7].key.string_value());
  ASSERT_TRUE(props[7].value.is_string_value());
  ASSERT_EQ("pepperoni pizza", props[7].value.string_value());
  ASSERT_EQ(parent::PIZZA_PEPPERONI, props[7].value.string_value());

  ASSERT_EQ("bindlibparent.codegen.testlib.Grit", props[8].key.string_value());
  ASSERT_EQ(parent::GRIT, props[8].key.string_value());
  ASSERT_TRUE(props[8].value.is_int_value());
  ASSERT_EQ(200u, props[8].value.int_value());
  ASSERT_EQ(lib::GRIT_FINE, props[8].value.int_value());
}
