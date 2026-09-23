// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/devicetree/testing/board-test-helper.h>

#include <gtest/gtest.h>

namespace devicetree_config {

namespace {

const zbi_platform_id_t kPlatformId = []() {
  zbi_platform_id_t plat_id = {};
  plat_id.vid = 0x11;
  plat_id.pid = 0x18;
  strcpy(plat_id.board_name, "devicetree-config");
  return plat_id;
}();

}  // namespace

class DevicetreeConfigTest : public testing::Test {
 public:
  DevicetreeConfigTest()
      : board_test_("/pkg/test-data/devicetree-config.dtb", kPlatformId, loop_.dispatcher(),
                    /*dtr_v2*/ true) {
    loop_.StartThread("test-realm");
    board_test_.SetupRealm();
  }

 protected:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  fdf_devicetree::testing::BoardTestHelper board_test_;
};

TEST_F(DevicetreeConfigTest, DevicetreeEnumeration) {
  std::vector<std::string> device_node_paths = {
      "sys/platform/devicetree-config-child",
  };
  ASSERT_TRUE(board_test_.StartRealm().is_ok());
  ASSERT_TRUE(board_test_.WaitOnDevices(device_node_paths).is_ok());
}

}  // namespace devicetree_config
