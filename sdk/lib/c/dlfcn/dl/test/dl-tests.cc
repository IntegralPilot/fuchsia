// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "../diagnostics.h"
#include "../stateful-error.h"

namespace {

// This file contains only the tests that should work on any platform.  These
// tests don't involve anything that needs an ELF file built, so they can run
// on non-ELF hosts.

TEST(DlTests, Diagnostics) {
  {
    dl::Diagnostics diag;
    fit::result<dl::Error, void*> result = diag.ok(nullptr);
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_EQ(result.value(), nullptr);
  }
  {
    dl::Diagnostics diag;
    fit::result<dl::Error> result = diag.ok();
    EXPECT_TRUE(result.is_ok()) << result.error_value();
  }
  {
    dl::Diagnostics diag;
    // Injects the prefix on diag.FormatError while in scope.
    ld::ScopedModuleDiagnostics module_diag{diag, "foo"};
    EXPECT_FALSE(diag.FormatError("some error", elfldltl::FileOffset{0x123u}));
    fit::result<dl::Error, int> result = diag.take_error();
    EXPECT_TRUE(result.is_error());
    EXPECT_EQ(result.error_value().take_str(), "foo: some error at file offset 0x123");
  }
  {
    dl::Diagnostics diag;
    {
      // No effect after it goes out of scope again.
      ld::ScopedModuleDiagnostics module_diag{diag, "foo"};
    }
    EXPECT_FALSE(diag.FormatError("some error"));
    fit::result<dl::Error> result = diag.take_error();
    EXPECT_TRUE(result.is_error());
    EXPECT_EQ(result.error_value().take_str(), "some error");
  }
}

TEST(DlTests, StatefuLError) {
  dl::StatefulError error_state;

  // Initially clear.
  EXPECT_EQ(error_state.GetAndClearLastError(), nullptr);

  // Called with success.
  EXPECT_EQ(error_state(fit::result<dl::Error, int>{fit::ok(3)}, -1), 3);

  // Still clear.
  EXPECT_EQ(error_state.GetAndClearLastError(), nullptr);

  // Called with error.
  EXPECT_EQ(error_state(fit::result<dl::Error, void*>{fit::error{dl::Error{"foo"}}}, nullptr),
            nullptr);

  // Returns error.
  EXPECT_STREQ(error_state.GetAndClearLastError(), "foo");

  // Clear after returning error.
  EXPECT_EQ(error_state.GetAndClearLastError(), nullptr);

  // Two errors without checking in between.
  EXPECT_EQ(error_state(fit::result<dl::Error, void*>{fit::error{dl::Error{"foo"}}}, nullptr),
            nullptr);
  EXPECT_EQ(error_state(fit::result<dl::Error, int>{fit::error{dl::Error{"bar"}}}, -1), -1);

  // Returns error.
  EXPECT_STREQ(error_state.GetAndClearLastError(), "bar");

  // Clear after returning error.
  EXPECT_EQ(error_state.GetAndClearLastError(), nullptr);
}

}  // namespace
