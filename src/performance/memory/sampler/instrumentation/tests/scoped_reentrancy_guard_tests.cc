// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>
#include <src/performance/memory/sampler/instrumentation/scoped_reentrancy_guard.h>

TEST(ScopedReentrancyGuardTest, ScopedReentrancyGuardWouldReenterSequence) {
  EXPECT_FALSE(memory_sampler::ScopedReentrancyGuard::WouldReenter());
  memory_sampler::ScopedReentrancyGuard guard;
  EXPECT_TRUE(memory_sampler::ScopedReentrancyGuard::WouldReenter());
}

TEST(ScopedReentrancyGuardTest, ScopedReentrancyGuardIsScoped) {
  EXPECT_FALSE(memory_sampler::ScopedReentrancyGuard::WouldReenter());
  {
    memory_sampler::ScopedReentrancyGuard guard;
    EXPECT_TRUE(memory_sampler::ScopedReentrancyGuard::WouldReenter());
  }
  EXPECT_FALSE(memory_sampler::ScopedReentrancyGuard::WouldReenter());
}
