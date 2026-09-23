// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/sysinfo.h>

#include <gtest/gtest.h>

namespace {

TEST(SysinfoTest, BasicSysinfo) {
  struct sysinfo info;
  ASSERT_EQ(sysinfo(&info), 0);
  EXPECT_GT(info.mem_unit, 0U);
  if (sizeof(info.totalram) == 8) {
    EXPECT_EQ(info.mem_unit, 1U);
  }
  EXPECT_GT(info.totalram, 0U);
  EXPECT_GT(info.freeram, 0U);
}

}  // namespace
