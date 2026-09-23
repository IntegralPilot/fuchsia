// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/netsvc/match.h"

#include <zxtest/zxtest.h>

TEST(MatchTest, Match) {
  constexpr char kInterface[] = "/dev/class/ethernet/group/foo/adapter/network";

  constexpr const char* kSuccessPatterns[] = {
      "/foo/adapter/network",
      "/ethernet/*foo*",
      "/ethernet/********foo**",
      "/foo/adapter/network*",
  };

  constexpr const char* kFailPatterns[] = {
      "/foo/adapter",
      "/ethernet/*/bar/*",
  };

  for (auto& pattern : kSuccessPatterns) {
    EXPECT_TRUE(EndsWithWildcardMatch(kInterface, pattern));
  }

  for (auto& pattern : kFailPatterns) {
    EXPECT_FALSE(EndsWithWildcardMatch(kInterface, pattern));
  }
}
