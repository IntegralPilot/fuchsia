// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/console_context.h"

#include <gtest/gtest.h>

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/mock_remote_api.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/console/console_test.h"

namespace zxdb {

namespace {

class ConsoleContextTest : public ConsoleTest {};

}  // namespace

// Verifies that the breakpoint is set to the current one when it's hit.
TEST_F(ConsoleContextTest, CurrentBreakpoint) {
  // Breakpoint 1.
  console().ProcessInputLine("b 0x10000");
  auto event = console().GetOutputEvent();
  ASSERT_TRUE(debug::StringContains(event.output.AsString(), "Created Breakpoint 1"))
      << event.output.AsString();
  int breakpoint_1_backend_id = mock_remote_api()->last_breakpoint_id();

  // Breakpoint 2.
  console().ProcessInputLine("b 0x20000");
  event = console().GetOutputEvent();
  ASSERT_TRUE(debug::StringContains(event.output.AsString(), "Created Breakpoint 2"))
      << event.output.AsString();

  // Breakpoint 2 should be active, so its address should be returned when we ask for the location.
  console().ProcessInputLine("bp get location");
  event = console().GetOutputEvent();
  ASSERT_TRUE(debug::StringContains(event.output.AsString(), "location = 0x20000"))
      << event.output.AsString();

  // Provide a stop at breakpoint 1.
  debug_ipc::NotifyException notify;
  notify.type = debug_ipc::ExceptionType::kSoftwareBreakpoint;
  notify.thread.id = {.process = kProcessKoid, .thread = kThreadKoid};
  notify.thread.state = debug_ipc::ThreadRecord::State::kBlocked;
  notify.thread.frames.emplace_back(0x10000, 0);
  notify.hit_breakpoints.emplace_back();
  notify.hit_breakpoints[0].id = breakpoint_1_backend_id;
  notify.hit_breakpoints[0].hit_count = 1;
  InjectException(notify);

  // Should have issued a stop at the first location.
  event = console().GetOutputEvent();
  ASSERT_TRUE(debug::StringContains(event.output.AsString(), "0x10000 (no symbol info)"))
      << event.output.AsString();

  // Breakpoint 1 should now be active.
  console().ProcessInputLine("bp get location");
  event = console().GetOutputEvent();
  ASSERT_TRUE(debug::StringContains(event.output.AsString(), "location = 0x10000"))
      << event.output.AsString();
}

// Testing EmbeddedMode requires us to do some configuration before creating the Console object,
// which we can't do if we use the ConsoleTest base class.
class EmbeddedModeConsoleContext : public RemoteAPITest {
 public:
  void SetUp() override {
    RemoteAPITest::SetUp();
    session().system().settings().SetString(ClientSettings::System::kConsoleMode,
                                            ClientSettings::System::kConsoleMode_Embedded);
    console_ = std::make_unique<MockConsole>(&session());
    console_->context().InitConsoleMode();
  }

  void TearDown() override {
    console_.reset();
    RemoteAPITest::TearDown();
  }

  MockConsole& console() const { return *console_; }

 private:
  std::unique_ptr<MockConsole> console_;
};

TEST_F(EmbeddedModeConsoleContext, ReturnToEmbeddedModeContinueThenQuit) {
  auto& context = console().context();

  // We should output nothing for the initial attach event.
  console().ProcessInputLine("attach xyz");
  EXPECT_FALSE(console().HasOutputEvent());

  constexpr uint64_t kProcessKoid = 1234;

  // Inject a process and break out of embedded mode.
  auto target = InjectProcess(kProcessKoid)->GetTarget();
  context.SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  // Do some inspection commands that should never leave interactive mode.
  console().ProcessInputLine("list");
  EXPECT_TRUE(console().HasOutputEvent());
  auto event = console().GetOutputEvent();  // Consume the event.
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  console().ProcessInputLine("b test");
  EXPECT_TRUE(console().HasOutputEvent());
  event = console().GetOutputEvent();
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  console().ProcessInputLine("continue");

  // We should still be in interactive mode.
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);
  EXPECT_EQ(target->GetState(), Target::State::kRunning);

  // Now we're really done, use "quit" to detach from everything.
  console().ProcessInputLine("quit");

  loop().RunUntilNoTasks();

  // Now we should be back in embedded mode because there are no other processes running.
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_Embedded);
}

TEST_F(EmbeddedModeConsoleContext, StayInteractiveWithContextualContinue) {
  auto& context = console().context();

  // We should output nothing for the initial attach event.
  console().ProcessInputLine("attach xyz");
  EXPECT_FALSE(console().HasOutputEvent());

  constexpr uint64_t kProcessKoid = 1234;
  constexpr uint64_t kThreadKoid = 4321;

  // Inject a process and break out of embedded mode.
  auto process = InjectProcess(kProcessKoid);
  auto target = process->GetTarget();
  auto thread = InjectThread(kProcessKoid, kThreadKoid);
  context.SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);
  context.SetActiveTarget(target);
  context.SetActiveThreadForTarget(thread);

  // Do some inspection commands that should never leave interactive mode.
  console().ProcessInputLine("b test");
  EXPECT_TRUE(console().HasOutputEvent());
  auto event = console().GetOutputEvent();  // Consume the event.
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  // Set up basic thread information so we can call "frame"
  debug_ipc::ThreadStatusReply reply;
  reply.record.id.process = kProcessKoid;
  reply.record.id.thread = kThreadKoid;
  reply.record.name = "thread1";
  reply.record.frames.emplace_back(0x1234, 0x5678);
  reply.record.frames.emplace_back(0x2345, 0x6789);
  reply.record.stack_amount = debug_ipc::ThreadRecord::StackAmount::kFull;
  reply.record.state = debug_ipc::ThreadRecord::State::kSuspended;
  mock_remote_api()->set_thread_status_reply(reply);

  // Now actually sync the stack with the backend.
  console().ProcessInputLine("frame");

  // Let all the remote api calls go through to propagate all the notifications.
  loop().RunUntilNoTasks();

  EXPECT_TRUE(console().HasOutputEvent());
  event = console().GetOutputEvent();
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  // A process specific continue, this may hit the breakpoint set during the interactive mode, or
  // could exit the process, but we should stay in interactive mode regardless.
  console().ProcessInputLine("process continue");

  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);
  EXPECT_EQ(target->GetState(), Target::State::kRunning);

  // We should see the same result with a thread level continue.
  console().ProcessInputLine("thread continue");

  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);
  EXPECT_EQ(target->GetState(), Target::State::kRunning);

  // Now end the process, which should bring us back to embedded mode since there are no other
  // running targets.
  target->Kill([](fxl::WeakPtr<Target> target, const Err& err) {
    debug::MessageLoop::Current()->QuitNow();
  });

  // Let the notifications propagate.
  loop().Run();

  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_Embedded);
  EXPECT_EQ(target->GetState(), Target::State::kNone);
}

TEST_F(EmbeddedModeConsoleContext, ReturnToEmbeddedModeOnQuitWithMultipleProcesses) {
  auto& context = console().context();

  // We should output nothing for the initial attach event.
  console().ProcessInputLine("attach xyz");
  EXPECT_FALSE(console().HasOutputEvent());

  constexpr uint64_t kProcess1Koid = 1234;
  constexpr uint64_t kProcess2Koid = 1235;

  // Inject two processes and break out of embedded mode.
  InjectProcess(kProcess1Koid)->GetTarget()->GetWeakPtr();
  InjectProcess(kProcess2Koid)->GetTarget()->GetWeakPtr();
  context.SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  // Do some inspection commands that should never leave interactive mode.
  console().ProcessInputLine("list");
  EXPECT_TRUE(console().HasOutputEvent());
  auto event = console().GetOutputEvent();  // Consume the event.
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  console().ProcessInputLine("b test");
  EXPECT_TRUE(console().HasOutputEvent());
  event = console().GetOutputEvent();
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  // Now quit to mark that we're done.
  console().ProcessInputLine("quit");

  loop().RunUntilNoTasks();

  // We should be back in embedded mode and attached to nothing.
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_Embedded);
  EXPECT_EQ(session().system().GetTargets().size(), 1u);
}

TEST_F(EmbeddedModeConsoleContext, ReturnToEmbeddedOnProcessExit) {
  auto& context = console().context();

  constexpr uint64_t kProcessKoid = 0x1234;
  constexpr uint64_t kThreadKoid = 0x4321;

  auto process = InjectProcess(kProcessKoid);
  auto thread = InjectThread(kProcessKoid, kThreadKoid);
  context.SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);
  context.SetActiveTarget(process->GetTarget());
  context.SetActiveThreadForTarget(thread);

  // Even using a process or thread specific continue will return to embedded mode when the process
  // exits.
  console().ProcessInputLine("process continue");

  // Should still be in interactive mode for now.
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  // Send the notification that the process exited successfully.
  debug_ipc::NotifyProcessExiting notify;
  notify.return_code = 0;
  notify.process_koid = kProcessKoid;
  session().DispatchNotifyProcessExiting(notify);

  loop().RunUntilNoTasks();

  // Now we should be back in embedded mode because there are no other processes running.
  EXPECT_EQ(context.GetConsoleMode(), ClientSettings::System::kConsoleMode_Embedded);
}

}  // namespace zxdb
