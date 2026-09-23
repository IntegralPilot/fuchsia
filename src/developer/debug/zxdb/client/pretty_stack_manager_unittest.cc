// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/pretty_stack_manager.h"

#include <lib/syslog/cpp/macros.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/mock_stack_delegate.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/lib/fxl/memory/ref_ptr.h"

namespace zxdb {

namespace {

std::vector<std::unique_ptr<Frame>> GetTestFrames() {
  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "Function1",
                                               FileLine("file1.cc", 23)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002, "Function2",
                                               FileLine("file2.cc", 23)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003, "Function3",
                                               FileLine("file3.cc", 23)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1004, 0x2004, "Function4",
                                               FileLine("file4.cc", 23)));
  return frames;
}

std::vector<std::unique_ptr<Frame>> GetCAsyncLoopFrames() {
  std::vector<std::unique_ptr<Frame>> frames;

  // C async_loop.
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "platform_syscall",
                                               FileLine("file1.c", 12)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002, "_zx_port_wait",
                                               FileLine("port.c", 21)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003,
                                               "async_loop_run_once", FileLine("loop.c", 22)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1004, 0x2004, "async_loop_run",
                                               FileLine("loop.c", 23)));
  return frames;
}

std::vector<std::unique_ptr<Frame>> GetCppAsyncLoopFrames() {
  auto frames = GetCAsyncLoopFrames();

  // This is just a thin wrapper around the C async loop implementation.
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1005, 0x2005, "async::Loop::Run",
                                               FileLine("loop.cc, ", 111)));
  return frames;
}

std::vector<std::unique_ptr<Frame>> GetRustAsyncExecutorPollResultFrames() {
  std::vector<std::unique_ptr<Frame>> frames;

  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "platform_syscall",
                                               FileLine("file1.c", 12)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002, "_zx_port_wait",
                                               FileLine("port.c", 12)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003,
                                               "zx::port::Port::wait", FileLine("port.rs", 12)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1004, 0x2004, "lambda1",
                                               FileLine("wait.rs", 12)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1005, 0x2005, "lambda2",
                                               FileLine("wait.rs", 12)));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1006, 0x2006,
      "std::thread::local::LocalKey<anything>::try_with<anything>", FileLine("local_key.rs", 12)));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1007, 0x2007, "std::thread::local::LocalKey<anything>::with<anything>",
      FileLine("local_key.rs", 12)));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1008, 0x2008,
      "fuchsia_async::runtime::fuchsia::executor::with_local_timer_heap<*>",
      FileLine("executor.rs", 12)));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1009, 0x2009,
      "fuchsia_async::runtime::fuchsia::executor::Executor::run_singlethreaded<*>",
      FileLine("executor.rs", 12)));

  return frames;
}

std::vector<std::unique_ptr<Frame>> GetRustPanickingFrames() {
  std::vector<std::unique_ptr<Frame>> frames;

  // This name is special, and we have to include it for the matcher to to match.
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "__abort_impl__", FileLine()));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002, "anything1", FileLine()));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003, "anything2",
                                               FileLine("rust_file.rs", 12)));
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x1004, 0x2004, "anything3", FileLine()));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1005, 0x2005,
                                               "std::sys::backtrace::__rust_end_short_backtrace<*>",
                                               FileLine()));

  return frames;
}

}  // namespace

TEST(PrettyStackManager, StackGlobMatchesAt) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  stack.SetFramesForTest(GetTestFrames(), true);

  // Single item that's not a match.
  PrettyStackManager::StackGlob single_unmatched("", {PrettyFrameGlob::Func("Unmatched")});
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(single_unmatched, stack, 0));
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(single_unmatched, stack, 1));
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(single_unmatched, stack, 2));

  // Single function that is a match.
  PrettyStackManager::StackGlob single_matched_func("", {PrettyFrameGlob::Func("Function2")});
  EXPECT_EQ(1u, PrettyStackManager::StackGlobMatchesAt(single_matched_func, stack, 1));

  // Single file that is a match.
  PrettyStackManager::StackGlob single_matched_file("", {PrettyFrameGlob::File("file2.cc")});
  EXPECT_EQ(1u, PrettyStackManager::StackGlobMatchesAt(single_matched_file, stack, 1));

  // Function match followed by file match.
  PrettyStackManager::StackGlob double_match(
      "", {PrettyFrameGlob::Func("Function2"), PrettyFrameGlob::File("file3.cc")});
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(double_match, stack, 0));
  EXPECT_EQ(2u, PrettyStackManager::StackGlobMatchesAt(double_match, stack, 1));

  // Wildcard that matches one thing at the beginning
  PrettyStackManager::StackGlob single_wildcard_beginning(
      "", {PrettyFrameGlob::Wildcard(1, 1), PrettyFrameGlob::File("file2.cc")});
  EXPECT_EQ(2u, PrettyStackManager::StackGlobMatchesAt(single_wildcard_beginning, stack, 0));
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(single_wildcard_beginning, stack, 1));

  // Wildcard that matches exactly two things in the middle.
  PrettyStackManager::StackGlob double_wildcard_middle(
      "", {PrettyFrameGlob::File("file1.cc"), PrettyFrameGlob::Wildcard(2, 2),
           PrettyFrameGlob::Func("Function4")});
  EXPECT_EQ(4u, PrettyStackManager::StackGlobMatchesAt(double_wildcard_middle, stack, 0));
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(double_wildcard_middle, stack, 1));

  // Wildcard that doesn't match.
  PrettyStackManager::StackGlob wildcard_miss(
      "", {PrettyFrameGlob::File("file1.cc"), PrettyFrameGlob::Wildcard(1, 1),
           PrettyFrameGlob::Func("Function4")});
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(wildcard_miss, stack, 0));

  // Wildcard that matches anything at the end. Since the wildcard matches as few items as possible,
  // it should match the minimum range when the wildcard is at the end of the glob list (in this
  // case 1 frame, giving 2 total frames matched).
  PrettyStackManager::StackGlob wildcard_end(
      "", {PrettyFrameGlob::File("file2.cc"), PrettyFrameGlob::Wildcard(1, 3)});
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(wildcard_end, stack, 0));
  EXPECT_EQ(2u, PrettyStackManager::StackGlobMatchesAt(wildcard_end, stack, 1));

  // Wildcard runs off the end.
  PrettyStackManager::StackGlob wildcard_off_end(
      "", {PrettyFrameGlob::File("file2.cc"), PrettyFrameGlob::Wildcard(4, 4)});
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(wildcard_off_end, stack, 1));

  // Wildcard requires too few items (but would otherwise match).
  PrettyStackManager::StackGlob wildcard_too_many(
      "", {PrettyFrameGlob::File("file1.cc"), PrettyFrameGlob::Wildcard(1, 1),
           PrettyFrameGlob::File("file4.cc")});
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(wildcard_too_many, stack, 1));

  // This wildcard allows (but doesn't require) running off the end but requires another entry after
  // it (that doesn't match in this case) so it won't match.
  PrettyStackManager::StackGlob wildcard_mismatch_off_end(
      "", {PrettyFrameGlob::File("file1.cc"), PrettyFrameGlob::Wildcard(1, 8),
           PrettyFrameGlob::File("NOT_PRESENT.cc")});
  EXPECT_EQ(0u, PrettyStackManager::StackGlobMatchesAt(wildcard_mismatch_off_end, stack, 0));
}

TEST(PrettyStackManager, GetMatchAt) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  stack.SetFramesForTest(GetTestFrames(), true);

  auto manager = fxl::MakeRefCounted<PrettyStackManager>();

  // Empty matchers should never match.
  EXPECT_FALSE(manager->GetMatchAt(stack, 0));
  EXPECT_FALSE(manager->GetMatchAt(stack, 1));
  EXPECT_FALSE(manager->GetMatchAt(stack, 2));
  EXPECT_FALSE(manager->GetMatchAt(stack, 3));

  // Supply two non-overlapping matchers.
  std::vector<PrettyStackManager::StackGlob> matchers;
  matchers.push_back(
      PrettyStackManager::StackGlob("Function1 Matcher", {PrettyFrameGlob::Func("Function1")}));
  matchers.push_back(
      PrettyStackManager::StackGlob("file2 Matcher", {PrettyFrameGlob::File("file2.cc")}));
  manager->SetMatchers(matchers);

  PrettyStackManager::Match result = manager->GetMatchAt(stack, 0);
  EXPECT_TRUE(result);
  EXPECT_EQ(1u, result.match_count);
  EXPECT_EQ("Function1 Matcher", result.description);

  result = manager->GetMatchAt(stack, 1);
  EXPECT_TRUE(result);
  EXPECT_EQ(1u, result.match_count);
  EXPECT_EQ("file2 Matcher", result.description);

  // Now add on top of that a wildcard match that covers more range, it should now be returned in
  // both cases since its longer.
  matchers.push_back(PrettyStackManager::StackGlob(
      "Star Matcher", {PrettyFrameGlob::Wildcard(1, 2), PrettyFrameGlob::File("file3.cc")}));
  manager->SetMatchers(matchers);

  result = manager->GetMatchAt(stack, 0);
  EXPECT_TRUE(result);
  EXPECT_EQ(3u, result.match_count);
  EXPECT_EQ("Star Matcher", result.description);

  result = manager->GetMatchAt(stack, 1);
  EXPECT_TRUE(result);
  EXPECT_EQ(2u, result.match_count);
  EXPECT_EQ("Star Matcher", result.description);
}

TEST(PrettyStackManager, ProcessStack) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  stack.SetFramesForTest(GetTestFrames(), true);

  auto manager = fxl::MakeRefCounted<PrettyStackManager>();

  // These two matchers have the same description, the PrettyStackManager should merge them.
  std::vector<PrettyStackManager::StackGlob> matchers;
  std::string matcher_name("Function1/2 Matcher");
  matchers.push_back(
      PrettyStackManager::StackGlob(matcher_name, {PrettyFrameGlob::Func("Function1")}));
  matchers.push_back(
      PrettyStackManager::StackGlob(matcher_name, {PrettyFrameGlob::Func("Function2")}));
  manager->SetMatchers(matchers);

  // Should get:
  //  0-1: "Function1/2 Matcher"
  //    2: Function3
  //    3: Function4
  std::vector<PrettyStackManager::FrameEntry> entries = manager->ProcessStack(stack);
  ASSERT_EQ(3u, entries.size());

  // The combined Function1/2 match.
  EXPECT_EQ(0u, entries[0].begin_index);
  EXPECT_EQ(matcher_name, entries[0].match.description);
  EXPECT_EQ(2u, entries[0].match.match_count);
  ASSERT_EQ(2u, entries[0].frames.size());
  EXPECT_EQ(stack[0], entries[0].frames[0]);
  EXPECT_EQ(stack[1], entries[0].frames[1]);

  // Function3 (bare).
  EXPECT_EQ(2u, entries[1].begin_index);
  EXPECT_FALSE(entries[1].match);
  ASSERT_EQ(1u, entries[1].frames.size());
  EXPECT_EQ(stack[2], entries[1].frames[0]);

  // Function4 (bare);
  EXPECT_EQ(3u, entries[2].begin_index);
  EXPECT_FALSE(entries[2].match);
  ASSERT_EQ(1u, entries[2].frames.size());
  EXPECT_EQ(stack[3], entries[2].frames[0]);
}

// Tests for a subset of the default matchers. These should all end up matching the entire given
// stack, resulting in a single entry returned from the PrettyStackManager.
TEST(PrettyStackManager, DefaultMatchersCpp) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  auto manager = fxl::MakeRefCounted<PrettyStackManager>();
  manager->LoadDefaultMatchers();

  stack.SetFramesForTest(GetCAsyncLoopFrames(), true);
  auto entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());

  stack.SetFramesForTest(GetCppAsyncLoopFrames(), true);
  entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());

  // pthread startup routines.
  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "start_pthread",
                                               FileLine("pthread.c", 11)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002,
                                               "thread_trampoline", FileLine("pthread.c", 12)));

  stack.SetFramesForTest(std::move(frames), true);
  entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());

  frames.clear();

  // Now match C++'s std::thread, which is called from the same pthread startup routines.
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "some_crazy_func",
                                               FileLine("thread", 10)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002, "start_pthread",
                                               FileLine("pthread.c", 11)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003,
                                               "thread_trampoline", FileLine("pthread.c", 12)));

  stack.SetFramesForTest(std::move(frames), true);
  entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());

  // Libc startup matchers.
  frames.clear();

  // Expect that we only have source info for the user's "main" function in a process's startup
  // thread.
  frames.push_back(
      std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001, "_start", FileLine("", 0)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002,
                                               "__libc_start_main", FileLine("", 0)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003, "main",
                                               FileLine("myfile.c", 10)));

  stack.SetFramesForTest(std::move(frames), true);
  entries = manager->ProcessStack(stack);

  // There are 2 entries since we don't elide the user's "main".
  EXPECT_EQ(entries.size(), 2u);
}

// Same as above but for Rust.
TEST(PrettyStackManager, DefaultMatchersRust) {
  Session session;
  MockStackDelegate delegate(&session);
  Stack stack(&delegate);
  delegate.set_stack(&stack);

  auto manager = fxl::MakeRefCounted<PrettyStackManager>();
  manager->LoadDefaultMatchers();

  // This matches when a thread is blocked in the async executor without any currently active tasks.
  stack.SetFramesForTest(GetRustAsyncExecutorPollResultFrames(), true);
  auto entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());

  // This matches when an async task has been selected to run.
  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1001, 0x2001,
      "fuchsia_async::runtime::fuchsia::executor::atomic_future::AtomicFuture<*>::poll<*>",
      FileLine("executor.rs", 1234)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002, "anything",
                                               FileLine("somewhere", 1234)));
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x100a, 0x200a,
      "fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run_singlethreaded<*>",
      FileLine("executor.rs", 1233)));

  stack.SetFramesForTest(std::move(frames), true);
  entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());

  // Matches a multithreaded executor in a test environment.
  frames.clear();
  frames.push_back(std::make_unique<MockFrame>(
      nullptr, nullptr, 0x1001, 0x2001,
      "fuchsia_async::runtime::fuchsia::executor::atomic_future::AtomicFuture<*>::poll<*>",
      FileLine("executor.rs", 1234)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002, "anything",
                                               FileLine("somewhere", 1234)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x100a, 0x200a,
                                               "fuchsia::test_singlethreaded<*>",
                                               FileLine("test.rs", 10)));

  stack.SetFramesForTest(std::move(frames), true);
  entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());

  // Matches the top of the backtrace code which runs when the Rust runtime encounters an exception.
  frames.clear();
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1001, 0x2001,
                                               "std::sys::backtrace::_print_fmt",
                                               FileLine("backtrace.rs", 1234)));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1002, 0x2002,
                                               "std::sys::backtrace::__rust_end_short_backtrace<*>",
                                               FileLine("backtrace.rs", 12)));
  // Need to add an unrelated frame outside of the matching part, since the matcher is allowed to
  // match a wildcard of length 0 but we have to have at least the same total number of frames as
  // globs. We could add this frame between the other two that are part of the matching pattern, but
  // this ensures we have the limits tested on the wildcard glob type.
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1003, 0x2003, "anything",
                                               FileLine("somewhere", 1234)));

  stack.SetFramesForTest(std::move(frames), true);
  entries = manager->ProcessStack(stack);
  // Because of the above, we'll have two for this one.
  ASSERT_EQ(2u, entries.size());

  frames.clear();

  // This is just default panicking frames that ends right at the "__rust_end_short_backtrace"
  // frame. We should collapse them all.
  // frames = GetRustPanickingFrames();
  stack.SetFramesForTest(GetRustPanickingFrames(), true);
  entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());

  frames.clear();

  // Now we take the default set of panicking frames and add two more on top of them after the
  // "__rust_end_short_backtrace" frame.
  frames = GetRustPanickingFrames();
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1006, 0x2006,
                                               "std::panicking::panic_handler", FileLine()));
  frames.push_back(std::make_unique<MockFrame>(nullptr, nullptr, 0x1006, 0x2006,
                                               "core::panicking::panic_fmt", FileLine()));
  stack.SetFramesForTest(std::move(frames), true);

  // This should also collapse into a single elided entry.
  entries = manager->ProcessStack(stack);
  ASSERT_EQ(1u, entries.size());
}

}  // namespace zxdb
