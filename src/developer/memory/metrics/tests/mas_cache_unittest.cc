// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/mas_cache.h"

#include <gtest/gtest.h>

namespace memory {
namespace {

TEST(MarkAndSweepCacheTest, BasicOperations) {
  MarkAndSweepCache<int, std::string> cache;

  EXPECT_FALSE(cache.Find(1).has_value());

  bool inserted = cache.Emplace(1, "one");
  EXPECT_TRUE(inserted);

  auto lookup_val = cache.Find(1);
  EXPECT_TRUE(lookup_val.has_value());
  EXPECT_EQ(*lookup_val, "one");

  auto missing_val = cache.Find(2);
  EXPECT_FALSE(missing_val.has_value());
}

TEST(MarkAndSweepCacheTest, SweepKeepsMarkedRemovesUnmarked) {
  MarkAndSweepCache<int, std::string> cache;

  cache.Emplace(1, "one");
  cache.Emplace(2, "two");

  // First sweep unmarks both.
  cache.Sweep();

  // Find 1, which marks it again.
  auto val = cache.Find(1);
  EXPECT_TRUE(val.has_value());

  // Sweep should remove 2 but keep 1.
  cache.Sweep();
  EXPECT_TRUE(cache.Find(1).has_value());
  EXPECT_FALSE(cache.Find(2).has_value());
}

TEST(MarkAndSweepCacheTest, EmplaceAlreadyExistingMarks) {
  MarkAndSweepCache<int, std::string> cache;

  cache.Emplace(1, "one");
  // Sweep to unmark A.
  cache.Sweep();

  // Emplace A again. It should return inserted=false, but mark it.
  bool inserted = cache.Emplace(1, "one_new");
  EXPECT_FALSE(inserted);

  // Sweep should keep 1.
  cache.Sweep();
  EXPECT_TRUE(cache.Find(1).has_value());
}

}  // namespace
}  // namespace memory
