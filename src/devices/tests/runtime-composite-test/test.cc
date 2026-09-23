// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/device-watcher/cpp/device-watcher.h>

#include <gtest/gtest.h>

TEST(SimpleDriverTestRealmTest, CompositeDriverConnectsToRuntimeProtocol) {
  ASSERT_EQ(device_watcher::RecursiveWaitForFile("/dev/sys/test/root").status_value(), ZX_OK);
  ASSERT_EQ(device_watcher::RecursiveWaitForFile("/dev/sys/test/leaf").status_value(), ZX_OK);
  ASSERT_EQ(
      device_watcher::RecursiveWaitForFile("/dev/sys/test/composite_fragment_a").status_value(),
      ZX_OK);
  ASSERT_EQ(
      device_watcher::RecursiveWaitForFile("/dev/sys/test/composite_fragment_b").status_value(),
      ZX_OK);
  ASSERT_EQ(device_watcher::RecursiveWaitForFile(
                "/dev/sys/test/composite_fragment_a/test_composite_1/composite")
                .status_value(),
            ZX_OK);
}
