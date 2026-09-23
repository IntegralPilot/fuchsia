// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace2json/trace_parser.h"

#include <fstream>
#include <sstream>
#include <vector>

#include <gtest/gtest.h>

namespace {

TEST(TraceParserTest, InvalidTrace) {
  std::istringstream input("asdfasdfasdfasdfasdf");
  const std::filesystem::path& output = "test-trace-invalid.json";
  tracing::FuchsiaTraceParser parser(output, "", {}, {});
  EXPECT_FALSE(parser.ParseComplete(&input));
}

// This is a regression test for https://fxbug.dev/42074400.  Check that
// EOF is handled properly.
//
// The actual bug occurred on a large trace file where an
// std::istream::read() call read exactly up to the end of the file,
// but we can reproduce the same problem with a zero-size file.
TEST(TraceParserTest, EndOfFile) {
  std::istringstream input("");
  const std::filesystem::path& output = "test-trace.json";
  ;

  tracing::FuchsiaTraceParser parser(output, "", {}, {});
  EXPECT_TRUE(parser.ParseComplete(&input));
}

}  // namespace
