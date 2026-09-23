// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_TEST_SANITIZER_TEST_UTILS_H_
#define LIB_C_TEST_SANITIZER_TEST_UTILS_H_

#include <lib/fdio/spawn.h>
#include <zircon/status.h>

#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

MATCHER(IsOk, "") {
  const bool ok = arg == ZX_OK;
  if (!ok) {
    *result_listener << zx_status_get_string(arg);
  }
  return ok;
}

#define ASSERT_OK(expr) ASSERT_THAT(expr, IsOk())
#define EXPECT_OK(expr) EXPECT_THAT(expr, IsOk())

struct HelperResult {
  friend constexpr decltype(auto) operator<<(auto& os, const HelperResult& result) {
    return os << "(exit: " << result.exit << ", out: \"" << result.out << "\", err: \""
              << result.err << "\")";
  }

  int64_t exit = -1;
  std::string out, err;
};

HelperResult RunHelper(std::string_view name, std::vector<const char*> args = {},
                       std::vector<fdio_spawn_action_t> = {});

// EXPECT_THAT(RunHelper("foo-helper"), ExitsWith(0, "", ""));
MATCHER_P3(ExitsWith, exit, out, err,
           ("(exit: " + ::testing::DescribeMatcher<int64_t>(exit) +
            ", out: " + ::testing::DescribeMatcher<std::string>(out) +
            ", err: " + ::testing::DescribeMatcher<std::string>(err) + ")")) {
  return ::testing::ExplainMatchResult(exit, arg.exit, result_listener) &&
         ::testing::ExplainMatchResult(out, arg.out, result_listener) &&
         ::testing::ExplainMatchResult(err, arg.err, result_listener);
}

#endif  // LIB_C_TEST_SANITIZER_TEST_UTILS_H_
