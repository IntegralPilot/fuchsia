// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_quit.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/remote_api_test.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/console/mock_console.h"

namespace zxdb {

namespace {

class VerbQuit : public RemoteAPITest {};

}  // namespace

// Quit with no running processes should exit immediately.
TEST_F(VerbQuit, QuitNoProcs) {
  MockConsole console(&session());
  console.Init();

  EXPECT_FALSE(console.has_quit());
  console.ProcessInputLine("quit");
  EXPECT_TRUE(console.has_quit());
}

// Quit with running processes should prompt.
TEST_F(VerbQuit, QuitRunningProcs) {
  MockConsole console(&session());
  console.Init();

  InjectProcess(1234);
  console.FlushOutputEvents();  // Process attaching will output some stuff.

  // This should prompt instead of quitting.
  console.ProcessInputLine("quit");
  EXPECT_FALSE(console.has_quit());

  auto output = console.GetOutputEvent();
  ASSERT_EQ(output.type, MockConsole::OutputEvent::Type::kOutput);
  EXPECT_EQ("\nAre you sure you want to quit and detach from the running process?\n",
            output.output.AsString());

  EXPECT_TRUE(console.SendModalReply("y"));
  EXPECT_TRUE(console.has_quit());
}

TEST_F(VerbQuit, QuitInEmbeddedMode) {
  // Enable Embedded mode before initializing the ConsoleContext console mode.
  session().system().settings().SetString(ClientSettings::System::kConsoleMode,
                                          ClientSettings::System::kConsoleMode_Embedded);
  MockConsole console(&session());
  console.Init();

  constexpr uint64_t kProcessKoid = 5678;
  // Should produce no output events since we're starting in embedded mode.
  auto target = InjectProcess(kProcessKoid)->GetTarget();
  EXPECT_FALSE(console.HasOutputEvent());

  // Transition to EmbeddedInteractive mode, which enables input and output.
  console.context().SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  // Typing "quit" should return us to embedded mode instead of quitting.
  console.ProcessInputLine("quit");

  // Run the loop to let the expected detach happen.
  loop().RunUntilNoTasks();

  // We should be back in embedded mode, and the console shouldn't think we want to quit.
  EXPECT_FALSE(console.has_quit());
  EXPECT_EQ(console.context().GetConsoleMode(), ClientSettings::System::kConsoleMode_Embedded);
  // Should be detached.
  EXPECT_EQ(target->GetState(), Target::State::kNone);

  // Now we're back in embedded mode, using "quit --force" should always quit regardless of embedded
  // mode.
  console.ProcessInputLine("quit --force");
  EXPECT_TRUE(console.has_quit());
}

}  // namespace zxdb
