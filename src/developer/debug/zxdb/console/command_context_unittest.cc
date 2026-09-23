// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/command_context.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/mock_remote_api.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/console/console_test.h"

namespace zxdb {

TEST(CommandContext, Empty) {
  bool called = false;
  {
    auto my_context = fxl::MakeRefCounted<OfflineCommandContext>(
        nullptr, [&called](OutputBuffer output, std::vector<Err> errors) {
          called = true;
          EXPECT_TRUE(output.AsString().empty());
          EXPECT_TRUE(errors.empty());
        });
  }
  EXPECT_TRUE(called);
}

TEST(CommandContext, AsyncOutputAndErrors) {
  // This test constructs two AsyncOutputBuffers, one depending on the other. We only keep a
  // reference to the inner one. The ConsoleContext should keep the reference to the outer one
  // to keep it alive as long as it's not complete.
  auto inner_async_output = fxl::MakeRefCounted<AsyncOutputBuffer>();

  bool called = false;
  {
    auto my_context = fxl::MakeRefCounted<OfflineCommandContext>(
        nullptr, [&called](OutputBuffer output, std::vector<Err> errors) {
          called = true;

          EXPECT_EQ("Some error\nSome output\nAsync output\n", output.AsString());

          EXPECT_EQ(1u, errors.size());
          EXPECT_EQ("Some error", errors[0].msg());
        });

    my_context->ReportError(Err("Some error"));
    my_context->Output("Some output\n");

    auto outer_async_output = fxl::MakeRefCounted<AsyncOutputBuffer>();
    outer_async_output->Append(inner_async_output);
    my_context->Output(outer_async_output);
    outer_async_output->Complete();
  }
  // Even though our reference went out-of-scope, the async output is still active.
  EXPECT_FALSE(called);

  inner_async_output->Append("Async output\n");
  inner_async_output->Complete();

  // Marking the async output complete should have marked the context done.
  EXPECT_TRUE(called);
}

class CommandContextTest : public ConsoleTest {};

TEST_F(CommandContextTest, CommandReport) {
  auto cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(&console());

  // Invalid command should not have anything set other than the error. Parsing is done
  // synchronously, so we don't need to run the message loop here.
  console().ProcessInputLine("1234", cmd_context);
  auto report = cmd_context->GetCommandReport();
  EXPECT_TRUE(cmd_context->has_error());
  EXPECT_TRUE(report.err.has_error());
  EXPECT_EQ(report.err.type(), ErrType::kGeneral);
  EXPECT_EQ(report.err.msg(), "The string \"1234\" is not a valid verb. Did you mean \"bp\"?");
  EXPECT_EQ(static_cast<Verb>(report.verb_id), Verb::kNone);
  EXPECT_TRUE(report.arguments.empty());
  EXPECT_TRUE(report.nouns.empty());
  EXPECT_TRUE(report.switches.empty());

  // Release the previous command context and create a new one for a clean slate.
  cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(&console());

  debug_ipc::ThreadRecord record;
  record.id.process = process()->GetKoid();
  record.id.thread = thread()->GetKoid();
  record.blocked_reason = debug_ipc::ThreadRecord::BlockedReason::kException;
  record.stack_amount = debug_ipc::ThreadRecord::StackAmount::kNone;

  debug_ipc::ThreadStatusReply reply;
  reply.record = record;
  mock_remote_api()->set_thread_status_reply(reply);

  // A verb with a single argument and no switches. Note the canonical name is always used in the
  // report.
  console().ProcessInputLine("b test.cc:1234", cmd_context);

  loop().RunUntilNoTasks();

  // Grab the report after all tasks are run.
  report = cmd_context->GetCommandReport();
  EXPECT_FALSE(cmd_context->has_error());
  EXPECT_FALSE(report.err.has_error());
  EXPECT_EQ(report.arguments.size(), 1u);
  EXPECT_TRUE(report.nouns.empty());
  EXPECT_TRUE(report.switches.empty());
  EXPECT_EQ(report.verb, "break");
  EXPECT_EQ(static_cast<Verb>(report.verb_id), Verb::kBreak);
  EXPECT_EQ(static_cast<CommandGroup>(report.command_group), CommandGroup::kBreakpoint);
  EXPECT_EQ(report.arguments[0], "test.cc:1234");

  cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(&console());

  // Now add a switch.
  console().ProcessInputLine("attach --weak component.cm", cmd_context);

  loop().RunUntilNoTasks();

  report = cmd_context->GetCommandReport();
  EXPECT_FALSE(cmd_context->has_error());
  EXPECT_FALSE(report.err.has_error());
  EXPECT_EQ(report.arguments.size(), 1u);
  EXPECT_TRUE(report.nouns.empty());
  EXPECT_EQ(report.switches.size(), 1u);
  EXPECT_EQ(report.verb, "attach");
  EXPECT_EQ(static_cast<Verb>(report.verb_id), Verb::kAttach);
  EXPECT_EQ(static_cast<CommandGroup>(report.command_group), CommandGroup::kProcess);
  EXPECT_EQ(report.arguments[0], "component.cm");
  EXPECT_EQ(report.switches[0].name, "weak");

  cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(&console());

  // Just nouns, again canonical names are always used, and mixing and matching is acceptable.
  console().ProcessInputLine("t * frame", cmd_context);

  loop().RunUntilNoTasks();

  report = cmd_context->GetCommandReport();
  EXPECT_FALSE(cmd_context->has_error());
  EXPECT_FALSE(report.err.has_error());
  EXPECT_TRUE(report.arguments.empty());
  EXPECT_EQ(report.nouns.size(), 2u);
  EXPECT_TRUE(report.switches.empty());
  EXPECT_EQ(report.verb, "");
  EXPECT_EQ(static_cast<Verb>(report.verb_id), Verb::kNone);
  for (const auto& noun : report.nouns) {
    auto it = GetNouns().find(static_cast<Noun>(noun.id));
    ASSERT_NE(it, GetNouns().end());
    auto noun_record = it->second;

    // Should always match the canonical name.
    EXPECT_EQ(noun_record.aliases[0], noun.name);

    // We gave the thread noun an index, but not frame.
    if (static_cast<Noun>(noun.id) == Noun::kThread) {
      EXPECT_EQ(noun.index, Command::kWildcard);
    } else {
      EXPECT_EQ(noun.index, Command::kNoIndex);
    }
  }

  cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(&console());

  debug_ipc::PauseReply pause_reply;
  pause_reply.threads.push_back(record);

  mock_remote_api()->set_pause_reply(pause_reply);

  // Noun prefix to a verb.
  console().ProcessInputLine("thread 1 pause", cmd_context);

  loop().RunUntilNoTasks();

  report = cmd_context->GetCommandReport();
  EXPECT_FALSE(cmd_context->has_error());
  EXPECT_FALSE(report.err.has_error());
  EXPECT_TRUE(report.arguments.empty());
  EXPECT_TRUE(report.switches.empty());
  EXPECT_EQ(report.nouns.size(), 1u);
  EXPECT_EQ(report.verb, "pause");
  EXPECT_EQ(static_cast<Verb>(report.verb_id), Verb::kPause);
  EXPECT_EQ(static_cast<Noun>(report.nouns[0].id), Noun::kThread);
  EXPECT_EQ(report.nouns[0].name, "thread");
  EXPECT_EQ(report.nouns[0].index, 1);
}

TEST_F(CommandContextTest, CommandReportElidesPaths) {
  auto cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(&console());

  console().ProcessInputLine("set symbol-cache=path/to/cache", cmd_context);

  // The command is parsed synchronously, but the actual actions will happen asynchronously.
  loop().RunUntilNoTasks();

  // Grab the report now that all of the async tasks have been completed.
  auto report = cmd_context->GetCommandReport();
  EXPECT_FALSE(cmd_context->has_error());
  EXPECT_FALSE(report.err.has_error());
  EXPECT_TRUE(report.arguments.empty());
  EXPECT_TRUE(report.switches.empty());
  EXPECT_TRUE(report.nouns.empty());
  EXPECT_EQ(report.verb, "set");
  EXPECT_EQ(static_cast<Verb>(report.verb_id), Verb::kSet);

  // Drop the last context and make a new one.
  cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(&console());

  console().ProcessInputLine("opendump path/to/minidump", cmd_context);
  loop().RunUntilNoTasks();

  // The command will fail because that path doesn't exist, but the rest of the command report will
  // be populated.
  report = cmd_context->GetCommandReport();
  EXPECT_TRUE(cmd_context->has_error());
  EXPECT_TRUE(report.err.has_error());
  EXPECT_TRUE(report.arguments.empty());
  EXPECT_TRUE(report.switches.empty());
  EXPECT_TRUE(report.nouns.empty());
  EXPECT_EQ(report.verb, "opendump");
  EXPECT_EQ(static_cast<Verb>(report.verb_id), Verb::kOpenDump);

  debug_ipc::ThreadRecord record;
  record.id.process = process()->GetKoid();
  record.id.thread = thread()->GetKoid();
  record.blocked_reason = debug_ipc::ThreadRecord::BlockedReason::kException;
  record.stack_amount = debug_ipc::ThreadRecord::StackAmount::kNone;

  debug_ipc::ThreadStatusReply reply;
  reply.record = record;

  debug_ipc::NotifyException notify;
  notify.thread = record;

  InjectException(notify);
  loop().RunUntilNoTasks();

  cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(&console());

  console().ProcessInputLine("savedump path/to/minidump", cmd_context);

  loop().RunUntilNoTasks();

  // This will also fail, but we're only interested in the command report.
  report = cmd_context->GetCommandReport();
  EXPECT_TRUE(cmd_context->has_error());
  EXPECT_TRUE(report.err.has_error());
  EXPECT_TRUE(report.arguments.empty());
  EXPECT_TRUE(report.switches.empty());
  EXPECT_TRUE(report.nouns.empty());
  EXPECT_EQ(report.verb, "savedump");
  EXPECT_EQ(static_cast<Verb>(report.verb_id), Verb::kSaveDump);
}

}  // namespace zxdb
