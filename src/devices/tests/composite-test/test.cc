// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/device-watcher/cpp/device-watcher.h>

#include <gtest/gtest.h>

// TODO(b/316176095): Re-enable test after ensuring it works with DFv2.
TEST(SimpleDriverTestRealmTest, DISABLED_DriversExist) {
  ASSERT_EQ(device_watcher::RecursiveWaitForFile("/dev/sys/test/root").status_value(), ZX_OK);
  ASSERT_EQ(device_watcher::RecursiveWaitForFile("/dev/sys/test/leaf").status_value(), ZX_OK);
  ASSERT_EQ(
      device_watcher::RecursiveWaitForFile("/dev/sys/test/node_group_fragment_a_1").status_value(),
      ZX_OK);
  ASSERT_EQ(
      device_watcher::RecursiveWaitForFile("/dev/sys/test/node_group_fragment_b_1").status_value(),
      ZX_OK);
  ASSERT_EQ(
      device_watcher::RecursiveWaitForFile("/dev/sys/test/node_group_fragment_a_2").status_value(),
      ZX_OK);
  ASSERT_EQ(
      device_watcher::RecursiveWaitForFile("/dev/sys/test/node_group_fragment_b_2").status_value(),
      ZX_OK);
  ASSERT_EQ(
      device_watcher::RecursiveWaitForFile("/dev/sys/test/node_group_fragment_c_2").status_value(),
      ZX_OK);
  ASSERT_EQ(device_watcher::RecursiveWaitForFile(
                "/dev/sys/test/node_group_fragment_a_1/test_composite_1/node_group")
                .status_value(),
            ZX_OK);
  ASSERT_EQ(device_watcher::RecursiveWaitForFile(
                "/dev/sys/test/node_group_fragment_a_2/test_composite_2/node_group")
                .status_value(),
            ZX_OK);
}
