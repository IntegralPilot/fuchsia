// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/shared/string_util.h"

#include <gtest/gtest.h>

namespace debug {

TEST(StringUtil, StringStartsWith) {
  EXPECT_FALSE(StringStartsWith("short", "much too long"));
  EXPECT_FALSE(StringStartsWith("a", "b"));
  EXPECT_FALSE(StringStartsWith("aaa", "b"));
  EXPECT_TRUE(StringStartsWith("abbb", "a"));
  EXPECT_TRUE(StringStartsWith("aabcde", "aabc"));
  EXPECT_TRUE(StringStartsWith("bcde", "bcde"));
  EXPECT_TRUE(StringStartsWith("aaab", ""));
}

TEST(StringUtil, StringEndsWith) {
  EXPECT_FALSE(StringEndsWith("short", "much too long"));
  EXPECT_FALSE(StringEndsWith("a", "b"));
  EXPECT_FALSE(StringEndsWith("aaa", "b"));
  EXPECT_TRUE(StringEndsWith("aaab", "b"));
  EXPECT_TRUE(StringEndsWith("aaabcde", "bcde"));
  EXPECT_TRUE(StringEndsWith("bcde", "bcde"));
  EXPECT_TRUE(StringEndsWith("aaab", ""));
}

TEST(StringUtil, StringContains) {
  EXPECT_FALSE(StringContains("short", "much too long"));
  EXPECT_FALSE(StringContains("a", "b"));
  EXPECT_FALSE(StringContains("aaa", "b"));
  EXPECT_TRUE(StringContains("aaab", "b"));
  EXPECT_TRUE(StringContains("aaabcde", "bcde"));
  EXPECT_TRUE(StringContains("bcde", "bcde"));
  EXPECT_TRUE(StringContains("aaab", ""));
}

}  // namespace debug
