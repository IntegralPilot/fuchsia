// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_TEST_FAILURE_STACK_MATCHER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_TEST_FAILURE_STACK_MATCHER_H_

#include "src/developer/debug/zxdb/client/pretty_stack_manager.h"
#include "src/developer/debug/zxdb/client/stack.h"

namespace zxdb {

// When running embedded within "fx test" or similar, we want to drop the user into their own
// test code, rather than deep in the test failure code that actually produced the stop. With that
// in mind, this class provides a simple wrapper around a PrettyStackManager that will match against
// frames corresponding to test failures in supported test frameworks.
class TestFailureStackMatcher : public fxl::RefCountedThreadSafe<TestFailureStackMatcher> {
 public:
  // Construct with fxl::MakeRefCounted().

  // Analyze |stack| for test failure frames. If one is found, the index of the next frame will be
  // returned, this is the frame that contains the user's code. Otherwise, `0u` is returned.
  size_t Match(const Stack& stack) const;

 private:
  FRIEND_REF_COUNTED_THREAD_SAFE(TestFailureStackMatcher);
  FRIEND_MAKE_REF_COUNTED(TestFailureStackMatcher);

  TestFailureStackMatcher();

  fxl::RefPtr<PrettyStackManager> pretty_stack_manager_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_TEST_FAILURE_STACK_MATCHER_H_
