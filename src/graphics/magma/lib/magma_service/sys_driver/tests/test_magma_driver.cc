// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/msd.h>
#include <lib/magma_service/test_util/platform_device_helper.h>

#include <gtest/gtest.h>

namespace msd {
class TestMagmaDriver {
 public:
  static void CreateAndDestroy() {
    auto driver = msd::Driver::MsdCreate();
    EXPECT_NE(nullptr, driver);
  }
};

TEST(MagmaDriver, CreateAndDestroy) { TestMagmaDriver::CreateAndDestroy(); }

}  // namespace msd
