// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/source_span.h"
#include "tools/fidl/fidlc/src/virtual_source_file.h"

namespace fidlc {
namespace {

TEST(VirtualSourceTests, AddLine) {
  VirtualSourceFile file("imaginary-test-file");

  SourceSpan one = file.AddLine("one");
  SourceSpan two = file.AddLine("two");
  SourceSpan three = file.AddLine("three");

  EXPECT_EQ(one.data(), "one");
  EXPECT_EQ(two.data(), "two");
  EXPECT_EQ(three.data(), "three");
}

TEST(VirtualSourceTests, LineContaining) {
  VirtualSourceFile file("imaginary-test-file");

  file.AddLine("one");
  SourceSpan two = file.AddLine("two");
  file.AddLine("three");

  SourceFile::Position pos{};
  file.LineContaining(two.data(), &pos);
  EXPECT_EQ(pos.line, 2);
  EXPECT_EQ(pos.column, 1);
}

}  // namespace
}  // namespace fidlc
