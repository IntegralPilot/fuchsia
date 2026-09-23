// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/msd.h>

#include <gtest/gtest.h>

TEST(MsdDriver, CreateAndDestroy) {
  auto msd_driver = msd::Driver::MsdCreate();
  ASSERT_NE(msd_driver, nullptr);
  msd_driver.reset();
}
