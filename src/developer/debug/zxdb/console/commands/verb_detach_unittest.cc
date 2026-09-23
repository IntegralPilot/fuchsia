// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_detach.h"

#include <gtest/gtest.h>

#include "src/developer/debug/shared/zx_status.h"
#include "src/developer/debug/zxdb/client/mock_remote_api.h"
#include "src/developer/debug/zxdb/client/remote_api_test.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/console/mock_console.h"
#include "src/developer/debug/zxdb/console/nouns.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {
namespace {

class TestRemoteAPI : public MockRemoteAPI {
 public:
  void Detach(const debug_ipc::DetachRequest& request,
              fit::callback<void(const Err&, debug_ipc::DetachReply)> cb) override {
    detaches_.push_back(request);

    debug_ipc::DetachReply reply;
    reply.status = debug::Status();

    cb(Err(), reply);
  }

  const std::vector<debug_ipc::DetachRequest>& detaches() const { return detaches_; }

 private:
  std::vector<debug_ipc::DetachRequest> detaches_;
};

class VerbDetach : public RemoteAPITest {
 public:
  TestRemoteAPI* remote_api() const { return remote_api_; }

 protected:
  std::unique_ptr<RemoteAPI> GetRemoteAPIImpl() override {
    auto remote_api = std::make_unique<TestRemoteAPI>();
    remote_api_ = remote_api.get();
    return remote_api;
  }

 private:
  TestRemoteAPI* remote_api_;
};

}  // namespace

TEST_F(VerbDetach, Detach) {
  MockConsole console(&session());
  console.Init();

  auto targets = session().system().GetTargetImpls();
  ASSERT_EQ(targets.size(), 1u);

  constexpr uint64_t kProcessKoid = 1;
  const std::string kProcessName = "process-1";

  targets[0]->CreateProcessForTesting(kProcessKoid, kProcessName);

  console.ProcessInputLine("detach");

  // Should've received a detach command.
  ASSERT_EQ(remote_api()->detaches().size(), 1u);
  EXPECT_EQ(remote_api()->detaches()[0].koid, kProcessKoid);

  // Specific detach should work.
  console.ProcessInputLine(fxl::StringPrintf("detach %" PRIu64, kProcessKoid));
  ASSERT_EQ(remote_api()->detaches().size(), 2u);
  EXPECT_EQ(remote_api()->detaches()[1].koid, kProcessKoid);

  // Some random detach should send a specific detach command.
  constexpr uint64_t kSomeOtherKoid = 0x1234;
  console.ProcessInputLine(fxl::StringPrintf("detach %" PRIu64, kSomeOtherKoid));
  ASSERT_EQ(remote_api()->detaches().size(), 3u);
  EXPECT_EQ(remote_api()->detaches()[2].koid, kSomeOtherKoid);
}

TEST_F(VerbDetach, DetachAll) {
  MockConsole console(&session());
  console.Init();

  constexpr uint64_t kProcess1Koid = 1;
  constexpr uint64_t kProcess2Koid = 2;
  constexpr uint64_t kProcess3Koid = 3;

  // Create 2 more targets to attach to.
  session().system().CreateNewTarget(nullptr);
  session().system().CreateNewTarget(nullptr);

  auto targets = session().system().GetTargetImpls();
  ASSERT_EQ(targets.size(), 3u);
  targets[0]->CreateProcessForTesting(kProcess1Koid, "process-1");
  targets[1]->CreateProcessForTesting(kProcess2Koid, "process-2");
  targets[2]->CreateProcessForTesting(kProcess3Koid, "process-3");

  console.ProcessInputLine("detach *");

  // Should have detached from everything.
  EXPECT_EQ(remote_api()->detaches().size(), 3u);
  EXPECT_EQ(remote_api()->detaches()[0].koid, kProcess1Koid);
  EXPECT_EQ(remote_api()->detaches()[1].koid, kProcess2Koid);
  EXPECT_EQ(remote_api()->detaches()[2].koid, kProcess3Koid);

  // There should only be the single required target remaining.
  targets = session().system().GetTargetImpls();
  EXPECT_EQ(targets.size(), 1u);
}

TEST_F(VerbDetach, DetachReturnsToEmbeddedMode) {
  MockConsole console(&session());
  // Enable Embedded mode before initializing the ConsoleContext console mode.
  session().system().settings().SetString(ClientSettings::System::kConsoleMode,
                                          ClientSettings::System::kConsoleMode_Embedded);
  console.Init();

  // Transition to EmbeddedInteractive mode, which enables input and output.
  console.context().SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  constexpr uint64_t kProcessKoid = 1;

  InjectProcess(kProcessKoid);

  // detach should detach from the process and return us to embedded mode.
  console.ProcessInputLine("detach");

  loop().RunUntilNoTasks();

  EXPECT_EQ(remote_api()->detaches().size(), 1u);
  EXPECT_EQ(console.context().GetConsoleMode(), ClientSettings::System::kConsoleMode_Embedded);
}

TEST_F(VerbDetach, DetachStaysInteractiveWithMultipleProcesses) {
  // Enable Embedded mode before initializing the ConsoleContext console mode.
  session().system().settings().SetString(ClientSettings::System::kConsoleMode,
                                          ClientSettings::System::kConsoleMode_Embedded);
  MockConsole console(&session());
  console.Init();

  constexpr uint64_t kProcess1Koid = 1;
  constexpr uint64_t kProcess2Koid = 2;
  constexpr uint64_t kProcess3Koid = 3;

  auto target1 = InjectProcess(kProcess1Koid)->GetTarget();
  // Take a WeakPtr so we can know if it got destructed.
  auto target2 = InjectProcess(kProcess2Koid)->GetTarget()->GetWeakPtr();

  // Transition to EmbeddedInteractive mode, which enables input and output.
  console.context().SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);
  console.context().SetActiveTarget(target1);
  auto active_target = console.context().GetActiveTarget();

  // This will detach from the active target.
  console.ProcessInputLine("detach");

  // Propagate all the notifications.
  loop().RunUntilNoTasks();

  // We should detach from the "active" process, and remain in EmbeddedInteractive mode, since there
  // is another attached process that is stopped.
  EXPECT_EQ(remote_api()->detaches().size(), 1u);
  EXPECT_EQ(console.context().GetConsoleMode(),
            ClientSettings::System::kConsoleMode_EmbeddedInteractive);
  EXPECT_EQ(active_target->GetState(), Target::State::kNone);

  auto target3 = InjectProcess(kProcess3Koid)->GetTarget()->GetWeakPtr();

  // |target2| should still be alive and non-destructed.
  ASSERT_TRUE(target2);
  ASSERT_TRUE(target3);
  EXPECT_EQ(target2->GetState(), Target::State::kRunning);
  EXPECT_EQ(target3->GetState(), Target::State::kRunning);

  console.context().SetActiveTarget(target3.get());

  // Grab the ID for the non-active Target.
  auto id = console.context().IdForTarget(target2.get());

  // Now detaching from the non-active console context target should only detach from that one, and
  // we should still be in EmbeddedInteractive mode.
  console.ProcessInputLine(fxl::StringPrintf("pr %d detach", id));

  // Propagate all the notifications.
  loop().RunUntilNoTasks();

  EXPECT_EQ(remote_api()->detaches().size(), 2u);
  EXPECT_EQ(console.context().GetConsoleMode(),
            ClientSettings::System::kConsoleMode_EmbeddedInteractive);
  ASSERT_TRUE(target2);
  EXPECT_EQ(target2->GetState(), Target::State::kNone);
}

TEST_F(VerbDetach, DetachAllReturnsToEmbedded) {
  // Enable Embedded mode before initializing the ConsoleContext console mode.
  session().system().settings().SetString(ClientSettings::System::kConsoleMode,
                                          ClientSettings::System::kConsoleMode_Embedded);
  MockConsole console(&session());
  console.Init();

  constexpr uint64_t kProcess1Koid = 1;
  constexpr uint64_t kProcess2Koid = 2;
  constexpr uint64_t kProcess3Koid = 3;

  InjectProcess(kProcess1Koid);
  InjectProcess(kProcess2Koid);
  InjectProcess(kProcess3Koid);

  // Transition to EmbeddedInteractive mode, which enables input and output.
  console.context().SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);

  console.ProcessInputLine("detach *");

  // Propagate all the notifications.
  loop().RunUntilNoTasks();

  // Everything should be detached and we should be back in embedded mode.
  EXPECT_EQ(remote_api()->detaches().size(), 3u);
  EXPECT_EQ(console.context().GetConsoleMode(), ClientSettings::System::kConsoleMode_Embedded);
  // Using the detach wildcard actually removes targets, which will resize the array.
  EXPECT_EQ(session().system().GetTargets().size(), 1u);
}

}  // namespace zxdb
