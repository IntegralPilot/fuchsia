// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/test_failure_stack_matcher.h"

#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/mock_stack_delegate.h"
#include "src/developer/debug/zxdb/client/session.h"

namespace zxdb {

namespace {

std::vector<std::unique_ptr<Frame>> GetGtestFrames() {
  std::vector<std::unique_ptr<Frame>> frames;

  // testing::UnitTest::AddTestPartResult
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1001, 0x2001, FileLine("file1.cc", 23),
      std::vector<std::string>{"testing", "UnitTest", "AddTestPartResult"}));

  // testing::internal::AssertHelper::operator=
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1002, 0x2002, FileLine("file2.cc", 23),
      std::vector<std::string>{"testing", "internal", "AssertHelper", "operator="}));

  // $anon::CrasherTest_ShouldFail_Test::TestBody
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1003, 0x2003, FileLine("file3.cc", 23),
      std::vector<std::string>{"$anon", "CrasherTest_ShouldFail_Test", "TestBody"}));

  // testing::internal::HandleSehExceptionsInMethodIfSupported<testing::Test, void>
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1004, 0x2004, FileLine("file4.cc", 23),
      std::vector<std::string>{"testing", "internal",
                               "HandleSehExceptionsInMethodIfSupported<testing::Test, void>"}));
  return frames;
}

std::vector<std::unique_ptr<Frame>> GetRustAssertEqFrames() {
  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "__abort_impl__",
                                               FileLine("file1.rs", 23)));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1002, 0x2002, FileLine("file2.rs", 23),
      std::vector<std::string>{"std", "sys", "pal", "unix", "abort_internal"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003, FileLine("file3.rs", 23),
                                  std::vector<std::string>{"std", "process", "abort"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x2004, 0x2004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic_handler"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x3004, 0x3004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic_handler"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x4004, 0x4004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic_handler"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x5004, 0x5004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic_handler"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x6004, 0x6004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic_handler"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x7004, 0x7004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic_handler"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x8004, 0x8004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic_handler"}));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1005, 0x2005, FileLine("file5.rs", 23),
      std::vector<std::string>{"core", "panicking", "assert_failed<i32, i32>"}));

  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1006, 0x2006, FileLine("file6.rs", 23),
      std::vector<std::string>{"rust_crasher_bin_test", "tests", "test_should_fail"}));
  return frames;
}

std::vector<std::unique_ptr<Frame>> GetRustAssertFrames() {
  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "__abort_impl__",
                                               FileLine("file1.rs", 23)));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1002, 0x2002, FileLine("file2.rs", 23),
      std::vector<std::string>{"std", "sys", "pal", "unix", "abort_internal"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003, FileLine("file3.rs", 23),
                                  std::vector<std::string>{"std", "process", "abort"}));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x2004, 0x2004, FileLine("file4.rs", 23),
      std::vector<std::string>{"test", "run_test_in_spawned_subprocess"}));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x3004, 0x3004, FileLine("file4.rs", 23),
      std::vector<std::string>{"std", "panicking", "begin_panic_handler"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x4004, 0x4004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic_fmt"}));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x5004, 0x5004, FileLine("file4.rs", 23),
                                  std::vector<std::string>{"core", "panicking", "panic"}));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1006, 0x2006, FileLine("file6.rs", 23),
      std::vector<std::string>{"fuchsia_test", "test", "empty_multithreaded_test_with_result",
                               "test_entry_point"}));
  return frames;
}

}  // namespace

TEST(TestFailureStackMatcher, MatchGtest) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  stack.SetFramesForTest(GetGtestFrames(), true);

  auto matcher = fxl::MakeRefCounted<TestFailureStackMatcher>();

  ASSERT_EQ(matcher->Match(stack), 2u);
}

TEST(TestFailureStackMatcher, MatchRustAssertEq) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  stack.SetFramesForTest(GetRustAssertEqFrames(), true);

  auto matcher = fxl::MakeRefCounted<TestFailureStackMatcher>();

  ASSERT_EQ(matcher->Match(stack), 11u);
}

TEST(TestFailureStackMatcher, MatchRustAssert) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  stack.SetFramesForTest(GetRustAssertFrames(), true);

  auto matcher = fxl::MakeRefCounted<TestFailureStackMatcher>();

  ASSERT_EQ(matcher->Match(stack), 7u);
}

TEST(TestFailureStackMatcher, NoMatch) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "some_function",
                                               FileLine("file1.cc", 11)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002, "other_function",
                                               FileLine("file2.rs", 12)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003, "my_function",
                                               FileLine("file3.cc", 21)));

  stack.SetFramesForTest(std::move(frames), true);

  auto matcher = fxl::MakeRefCounted<TestFailureStackMatcher>();
  ASSERT_EQ(matcher->Match(stack), 0u);
}

}  // namespace zxdb
