// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/format_thread.h"

#include <gtest/gtest.h>

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/console/console_context.h"
#include "src/developer/debug/zxdb/console/console_test.h"
#include "src/developer/debug/zxdb/symbols/file_line.h"

namespace zxdb {

namespace {

class FormatThread : public ConsoleTest {};

TEST_F(FormatThread, FormatThreadStopException) {
  std::vector<std::unique_ptr<Frame>> frames;
  frames.emplace_back(std::make_unique<MockFrame>(&session(), thread(), 0x1234, 0x2345,
                                                  FileLine("somefile.rs", 10),
                                                  std::vector<std::string>{"one", "two"}));
  frames.emplace_back(std::make_unique<MockFrame>(&session(), thread(), 0x1334, 0x2445,
                                                  FileLine("somefile.rs", 20),
                                                  std::vector<std::string>{"one", "three"}));
  frames.emplace_back(std::make_unique<MockFrame>(&session(), thread(), 0x1434, 0x2545,
                                                  FileLine("somefile.rs", 30),
                                                  std::vector<std::string>{"one", "four"}));
  frames.emplace_back(std::make_unique<MockFrame>(&session(), thread(), 0x1534, 0x2645,
                                                  FileLine("somefile.rs", 40),
                                                  std::vector<std::string>{"one", "five"}));

  debug_ipc::NotifyException exception;
  exception.type = debug_ipc::ExceptionType::kPageFault;
  exception.thread.id = {.process = process()->GetKoid(), .thread = thread()->GetKoid()};
  exception.thread.state = debug_ipc::ThreadRecord::State::kBlocked;
  exception.thread.blocked_reason = debug_ipc::ThreadRecord::BlockedReason::kException;
  exception.thread.frames.emplace_back(
      // Same values as |frames[0]|.
      debug_ipc::StackFrame(0x1234, 0x2345, 0, debug_ipc::StackFrame::Trust::kContext, {}));

  InjectExceptionWithStack(exception, std::move(frames), /*has_all_frames*/ true);

  auto event = console().GetOutputEvent();
  ASSERT_EQ(event.type, MockConsole::OutputEvent::Type::kOutput);

  auto output = event.output.AsString();
  EXPECT_EQ(
      output,
      "══════════════════════════\n"
      " No exception information\n"
      "══════════════════════════\n"
      " Process 1 (koid=875123541) thread 1 (koid=19028730)\n"
      " Faulting instruction: 0x1234\n"
      "\n"
      "🛑 one::two() • somefile.rs:10\n"
      "Source file somefile.rs not found. You might want to adjust the source file remap setting. See \"get source-map\".");
}

TEST_F(FormatThread, FormatThreadStopBreakpoint) {
  std::vector<std::unique_ptr<Frame>> frames;
  frames.emplace_back(std::make_unique<MockFrame>(&session(), thread(), 0x1234, 0x2345,
                                                  FileLine("somefile.rs", 10),
                                                  std::vector<std::string>{"one", "two"}));
  frames.emplace_back(std::make_unique<MockFrame>(&session(), thread(), 0x1334, 0x2445,
                                                  FileLine("somefile.rs", 20),
                                                  std::vector<std::string>{"one", "three"}));
  frames.emplace_back(std::make_unique<MockFrame>(&session(), thread(), 0x1434, 0x2545,
                                                  FileLine("somefile.rs", 30),
                                                  std::vector<std::string>{"one", "four"}));
  frames.emplace_back(std::make_unique<MockFrame>(&session(), thread(), 0x1534, 0x2645,
                                                  FileLine("somefile.rs", 40),
                                                  std::vector<std::string>{"one", "five"}));

  debug_ipc::NotifyException exception;
  exception.type = debug_ipc::ExceptionType::kSoftwareBreakpoint;
  exception.thread.id = {.process = process()->GetKoid(), .thread = thread()->GetKoid()};
  exception.thread.state = debug_ipc::ThreadRecord::State::kBlocked;
  exception.thread.blocked_reason = debug_ipc::ThreadRecord::BlockedReason::kException;
  exception.thread.frames.emplace_back(
      // Same values as |frames[0]|.
      debug_ipc::StackFrame(0x1234, 0x2345, 0, debug_ipc::StackFrame::Trust::kContext, {}));

  InjectExceptionWithStack(exception, std::move(frames), /*has_all_frames*/ true);

  auto event = console().GetOutputEvent();
  ASSERT_EQ(event.type, MockConsole::OutputEvent::Type::kOutput);

  auto output = event.output.AsString();
  EXPECT_EQ(
      output,
      "🛑 one::two() • somefile.rs:10\n"
      "Source file somefile.rs not found. You might want to adjust the source file remap setting. See \"get source-map\".");
}

TEST_F(FormatThread, FormatThreadConcise) {
  auto out = FormatThreadConcise(&console().context(), thread());

  EXPECT_EQ(out.AsString(), "Thread 1 state=Running koid=19028730 name=\"test 19028730\"");

  debug_ipc::NotifyException exception;
  exception.type = debug_ipc::ExceptionType::kPageFault;
  exception.thread.id = {.process = process()->GetKoid(), .thread = thread()->GetKoid()};
  exception.thread.name = thread()->GetName();
  exception.thread.state = debug_ipc::ThreadRecord::State::kBlocked;
  exception.thread.blocked_reason = debug_ipc::ThreadRecord::BlockedReason::kException;
  InjectException(exception);

  out.Clear();
  out = FormatThreadConcise(&console().context(), thread());
  EXPECT_EQ(out.AsString(),
            "Thread 1 state=\"Blocked (Exception)\" koid=19028730 name=\"test 19028730\"");
}

}  // namespace

}  // namespace zxdb
