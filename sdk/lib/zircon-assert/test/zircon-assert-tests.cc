// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

using ::testing::AllOf;
using ::testing::HasSubstr;
using ::testing::Not;

TEST(ZirconAssertTests, ZxPanic) {
  EXPECT_DEATH(
      {
        // This is not ZX_ASSERT(), since ZX_PANIC() is what we're testing.
        // ZX_ASSERT() et al just call ZX_PANIC() anyway.
        ZX_PANIC("This message should be seen on stderr.  %d", 42);
        ZX_PANIC("This message should not be seen on stderr.");
      },
      AllOf(HasSubstr("This message should be seen on stderr.  42\n"),
            Not(HasSubstr("This message should not be seen on stderr.\n"))));
}

}  // namespace
