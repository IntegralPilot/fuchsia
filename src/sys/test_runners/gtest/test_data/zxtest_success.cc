// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

namespace {

TEST(BasicTest, Basic) { ASSERT_TRUE(true); }

TEST(BasicTest, Basic2) { ASSERT_FALSE(false); }

TEST(BasicTest, DISABLED_DisabledTest) { ASSERT_TRUE(false); }

TEST(BasicTest, SkipMe) { ZXTEST_SKIP("Don't run this test anymore"); }

}  // namespace
