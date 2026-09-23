// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/test_failure_stack_matcher.h"

namespace zxdb {

TestFailureStackMatcher::TestFailureStackMatcher()
    : pretty_stack_manager_(fxl::MakeRefCounted<PrettyStackManager>()) {
  // These match the frame above the actual test code for Rust and gTest failure paths,
  // respectively. When either of these match a frame in the stack, the user's test code will be
  // the next frame. The function names are taken from the stack of a failed test from each test
  // framework. If additional test frameworks need to be added, they should be appended here.
  // Descriptions may be duplicated so if a particular framework has multiple failure paths, they
  // may all be added with the same description.
  pretty_stack_manager_->SetMatchers({
      // This matches Rust tests when an "assert_eq" fails.
      PrettyStackManager::StackGlob("Rust test assertion",
                                    {PrettyFrameGlob::Func("core::panicking::assert_failed<*>")}),
      // And this matches a plain old "assert".
      PrettyStackManager::StackGlob("Rust test assertion",
                                    {PrettyFrameGlob::Func("core::panicking::panic")}),
      PrettyStackManager::StackGlob(
          "gTest test assertion",
          {PrettyFrameGlob::Func("testing::internal::AssertHelper::operator=")}),
  });
}

size_t TestFailureStackMatcher::Match(const Stack& stack) const {
  size_t best_frame_index = 0;
  for (const auto& entry : pretty_stack_manager_->ProcessStack(stack)) {
    if (entry.match.match_count > 0) {
      best_frame_index = entry.begin_index + entry.frames.size();
    }
  }

  return best_frame_index;
}
}  // namespace zxdb
