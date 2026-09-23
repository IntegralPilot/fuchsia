// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/msd.h>
#include <lib/magma_service/test_util/platform_msd_device_helper.h>

#include <gtest/gtest.h>

TEST(MsdContext, CreateAndDestroy) {
  auto msd_driver = msd::Driver::MsdCreate();
  ASSERT_NE(msd_driver, nullptr);

  auto msd_device = msd_driver->MsdCreateDevice(GetTestDeviceHandle());
  ASSERT_NE(msd_device, nullptr);

  auto msd_connection = msd_device->MsdOpen(0);
  ASSERT_NE(msd_connection, nullptr);

  auto msd_context = msd_connection->MsdCreateContext();
  EXPECT_NE(msd_context, nullptr);

  msd_context.reset();
  msd_connection.reset();
  msd_device.reset();
  msd_driver.reset();
}
