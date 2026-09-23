// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/types.h>

#include <gtest/gtest.h>

#include "src/developer/memory/metrics/capture.h"

#define ASSERT_OK_AND_ASSIGN(name, result)                                                 \
  auto name##_result = result;                                                             \
  ASSERT_TRUE(name##_result.is_ok()) << zx_status_get_string(name##_result.error_value()); \
  auto name = result.value();

namespace memory::test {

// These tests are exercising the real system services. As such we can't assume much about exactly
// what is running and what the memory looks like. We're just vetting whether they return any data
// at all without error.
using CaptureSystemTest = testing::Test;

TEST_F(CaptureSystemTest, KMEM) {
  ASSERT_OK_AND_ASSIGN(capture_maker, memory::CaptureMaker::Create(CreateDefaultOS()));
  Capture c;
  auto ret = capture_maker.GetCapture(&c, CaptureLevel::KMEM);
  ASSERT_EQ(ZX_OK, ret) << zx_status_get_string(ret);
  EXPECT_LT(0U, c.kmem().free_bytes);
  EXPECT_LT(0U, c.kmem().total_bytes);
}

// TODO(https://fxbug.dev/42059717): deflake and reenable.
TEST_F(CaptureSystemTest, VMO) {
  ASSERT_OK_AND_ASSIGN(capture_maker, memory::CaptureMaker::Create(CreateDefaultOS()));
  Capture c;
  auto ret = capture_maker.GetCapture(&c, CaptureLevel::VMO);
  ASSERT_EQ(ZX_OK, ret) << zx_status_get_string(ret);
  EXPECT_LT(0U, c.kmem().free_bytes);
  EXPECT_LT(0U, c.kmem().total_bytes);

  ASSERT_LT(0U, c.koid_to_process().size());
}

}  // namespace memory::test
