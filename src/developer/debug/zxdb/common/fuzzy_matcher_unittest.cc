// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/common/fuzzy_matcher.h"

#include <gtest/gtest.h>

namespace zxdb {

TEST(FuzzyMatcher, MatchesLine) {
  FuzzyMatcher matcher("line 1\nline 2\nline 3");

  EXPECT_TRUE(matcher.MatchesLine("line 1", false));
  EXPECT_TRUE(matcher.MatchesLine("line 2", false));
  EXPECT_TRUE(matcher.MatchesLine("line 3", false));
  EXPECT_FALSE(matcher.MatchesLine("line 4", false));
}

TEST(FuzzyMatcher, Wildcard) {
  FuzzyMatcher matcher("Launched Process 1 state=Running koid=1234 name=test.cm component=test.cm");

  EXPECT_TRUE(matcher.MatchesLine(
      "Launched Process ?? state=Running koid=?? name=test.cm component=test.cm", false));
}

TEST(FuzzyMatcher, OutOfOrder) {
  FuzzyMatcher matcher("a\nb\nc");

  EXPECT_TRUE(matcher.MatchesLine("c", true));
  EXPECT_TRUE(matcher.MatchesLine("a", true));
}

}  // namespace zxdb
