// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/developer/debug/debug_agent/debug_agent.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/arch.h"
#include "src/developer/debug/debug_agent/mock_debug_agent_harness.h"
#include "src/developer/debug/debug_agent/mock_exception_handle.h"
#include "src/developer/debug/debug_agent/mock_job_handle.h"
#include "src/developer/debug/debug_agent/mock_process.h"
#include "src/developer/debug/debug_agent/mock_process_handle.h"
#include "src/developer/debug/debug_agent/mock_thread_handle.h"
#include "src/developer/debug/debug_agent/remote_api.h"
#include "src/developer/debug/debug_agent/test_utils.h"
#include "src/developer/debug/ipc/filter_utils.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/logging/debug.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/test_with_loop.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace debug_agent {
namespace {

bool HasAttachedProcessWithKoid(DebugAgent* debug_agent, zx_koid_t koid) {
  DebuggedProcess* proc = debug_agent->GetDebuggedProcess(koid);
  if (!proc)
    return false;

  // All of our process handles should be mock ones.
  return static_cast<MockProcessHandle&>(proc->process_handle()).IsAttached();
}

// Setup -------------------------------------------------------------------------------------------

class DebugAgentMockProcess : public MockProcess {
 public:
  DebugAgentMockProcess(DebugAgent* debug_agent, zx_koid_t koid, std::string name)
      : MockProcess(debug_agent, koid, std::move(name)) {}

  ~DebugAgentMockProcess() = default;

  void SuspendAndSendModules() override {
    // Send the modules over to the ipc.
    debug_agent()->SendNotification(modules_to_send_);
  }

  void set_modules_to_send(debug_ipc::NotifyModules m) { modules_to_send_ = std::move(m); }

 private:
  debug_ipc::NotifyModules modules_to_send_;
};

}  // namespace

// Tests -------------------------------------------------------------------------------------------

class DebugAgentTests : public debug::TestWithLoop {};

TEST_F(DebugAgentTests, OnGlobalStatus) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  debug_ipc::StatusRequest request = {};

  debug_ipc::StatusReply reply = {};
  remote_api->OnStatus(request, &reply);

  ASSERT_EQ(reply.processes.size(), 0u);

  constexpr uint64_t kProcessKoid1 = 0x1234;
  const std::string kProcessName1 = "process-1";
  constexpr uint64_t kProcess1ThreadKoid1 = 0x1;

  auto process1 = std::make_unique<MockProcess>(nullptr, kProcessKoid1, kProcessName1);
  process1->AddThread(kProcess1ThreadKoid1);
  harness.debug_agent()->InjectProcessForTest(std::move(process1));

  reply = {};
  remote_api->OnStatus(request, &reply);

  ASSERT_EQ(reply.processes.size(), 1u);
  EXPECT_EQ(reply.processes[0].process_koid, kProcessKoid1);
  EXPECT_EQ(reply.processes[0].process_name, kProcessName1);
  ASSERT_EQ(reply.processes[0].threads.size(), 1u);
  EXPECT_EQ(reply.processes[0].threads[0].id.process, kProcessKoid1);
  EXPECT_EQ(reply.processes[0].threads[0].id.thread, kProcess1ThreadKoid1);

  constexpr uint64_t kProcessKoid2 = 0x5678;
  const std::string kProcessName2 = "process-2";
  constexpr uint64_t kProcess2ThreadKoid1 = 0x1;
  constexpr uint64_t kProcess2ThreadKoid2 = 0x2;

  auto process2 = std::make_unique<MockProcess>(nullptr, kProcessKoid2, kProcessName2);
  process2->AddThread(kProcess2ThreadKoid1);
  process2->AddThread(kProcess2ThreadKoid2);
  harness.debug_agent()->InjectProcessForTest(std::move(process2));

  reply = {};
  remote_api->OnStatus(request, &reply);

  ASSERT_EQ(reply.processes.size(), 2u);
  EXPECT_EQ(reply.processes[0].process_koid, kProcessKoid1);
  EXPECT_EQ(reply.processes[0].process_name, kProcessName1);
  ASSERT_EQ(reply.processes[0].threads.size(), 1u);
  EXPECT_EQ(reply.processes[0].threads[0].id.process, kProcessKoid1);
  EXPECT_EQ(reply.processes[0].threads[0].id.thread, kProcess1ThreadKoid1);

  EXPECT_EQ(reply.processes[1].process_koid, kProcessKoid2);
  EXPECT_EQ(reply.processes[1].process_name, kProcessName2);
  ASSERT_EQ(reply.processes[1].threads.size(), 2u);
  EXPECT_EQ(reply.processes[1].threads[0].id.process, kProcessKoid2);
  EXPECT_EQ(reply.processes[1].threads[0].id.thread, kProcess2ThreadKoid1);
  EXPECT_EQ(reply.processes[1].threads[1].id.process, kProcessKoid2);
  EXPECT_EQ(reply.processes[1].threads[1].id.thread, kProcess2ThreadKoid2);

  // Set a limbo provider.

  constexpr zx_koid_t kProcKoid1 = 100;
  constexpr zx_koid_t kThreadKoid1 = 101;
  harness.system_interface()->mock_limbo_provider().AppendException(
      MockProcessHandle(kProcKoid1, "proc1"), MockThreadHandle(kThreadKoid1, "thread1"),
      MockExceptionHandle(kThreadKoid1));

  constexpr zx_koid_t kProcKoid2 = 102;
  constexpr zx_koid_t kThreadKoid2 = 103;
  harness.system_interface()->mock_limbo_provider().AppendException(
      MockProcessHandle(kProcKoid2, "proc2"), MockThreadHandle(kThreadKoid1, "thread2"),
      MockExceptionHandle(kThreadKoid2));

  reply = {};
  remote_api->OnStatus(request, &reply);

  // The attached processes should still be there.
  ASSERT_EQ(reply.processes.size(), 2u);

  // The limbo processes should be there.
  ASSERT_EQ(reply.limbo.size(), 2u);
  EXPECT_EQ(reply.limbo[0].process_koid, kProcKoid1);
  EXPECT_EQ(reply.limbo[0].process_name, "proc1");
  ASSERT_EQ(reply.limbo[0].threads.size(), 1u);

  // TODO(donosoc): Add exception type.
}

TEST_F(DebugAgentTests, OnAttachNotFound) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  debug_ipc::AttachRequest attach_request;
  attach_request.koid = -1;

  // Invalid koid should fail.
  debug_ipc::AttachReply attach_reply;
  remote_api->OnAttach(attach_request, &attach_reply);

  EXPECT_TRUE(attach_reply.status.has_error());

  constexpr zx_koid_t kProcKoid1 = 100;
  constexpr zx_koid_t kThreadKoid1 = 101;
  harness.system_interface()->mock_limbo_provider().AppendException(
      MockProcessHandle(kProcKoid1, "proc1"), MockThreadHandle(kThreadKoid1, "thread1"),
      MockExceptionHandle(kThreadKoid1));

  // Even with limbo it should fail.
  remote_api->OnAttach(attach_request, &attach_reply);
  EXPECT_TRUE(attach_reply.status.has_error());
}

TEST_F(DebugAgentTests, OnAttach) {
  constexpr zx_koid_t kProcess1Koid = 11u;  // Koid for job1-p2 from the GetMockJobTree() hierarchy.

  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  debug_ipc::AttachRequest attach_request;
  attach_request.koid = kProcess1Koid;

  debug_ipc::AttachReply attach_reply;
  remote_api->OnAttach(attach_request, &attach_reply);

  // We should've received a watch command (which does the low level exception watching).
  EXPECT_TRUE(HasAttachedProcessWithKoid(harness.debug_agent(), kProcess1Koid));

  // We should've gotten an attach reply.
  EXPECT_TRUE(attach_reply.status.ok());
  EXPECT_EQ(attach_reply.koid, kProcess1Koid);
  EXPECT_EQ(attach_reply.name, "job1-p2");

  // Asking for some invalid process should fail.
  attach_request.koid = 0x231315;  // Some invalid value.
  remote_api->OnAttach(attach_request, &attach_reply);

  // We should've gotten an error reply.
  EXPECT_TRUE(attach_reply.status.has_error());

  // Attaching to a third process should work.
  attach_request.koid = 21u;
  remote_api->OnAttach(attach_request, &attach_reply);

  EXPECT_TRUE(attach_reply.status.ok());
  EXPECT_EQ(attach_reply.koid, 21u);
  EXPECT_EQ(attach_reply.name, "job121-p2");

  // Attaching again to a process should fail.
  remote_api->OnAttach(attach_request, &attach_reply);

  EXPECT_TRUE(attach_reply.status.has_error());
}

TEST_F(DebugAgentTests, AttachToLimbo) {
  // debug::SetDebugMode(true);
  // debug::SetLogCategories({debug::LogCategory::kAll});

  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr zx_koid_t kProcKoid = 100;
  constexpr zx_koid_t kThreadKoid = 101;
  MockProcessHandle mock_process(kProcKoid, "proc");
  MockThreadHandle mock_thread(kThreadKoid, "thread");
  mock_process.set_threads({mock_thread});

  harness.system_interface()->mock_limbo_provider().AppendException(
      mock_process, mock_thread, MockExceptionHandle(kThreadKoid));

  debug_ipc::AttachRequest attach_request;
  attach_request.koid = kProcKoid;

  debug_ipc::AttachReply attach_reply;
  remote_api->OnAttach(attach_request, &attach_reply);
  // The threads are only populated on the next tick.
  debug::MessageLoop::Current()->RunUntilNoTasks();

  // Process should be watching.
  EXPECT_TRUE(HasAttachedProcessWithKoid(harness.debug_agent(), kProcKoid));

  // We should've gotten an attach reply.
  EXPECT_TRUE(attach_reply.status.ok());
  EXPECT_EQ(attach_reply.koid, kProcKoid);
  EXPECT_EQ(attach_reply.name, "proc");

  {
    DebuggedProcess* process = harness.debug_agent()->GetDebuggedProcess(kProcKoid);
    ASSERT_TRUE(process);
    auto threads = process->GetThreads();
    ASSERT_EQ(threads.size(), 1u);

    // Search for the exception thread.
    DebuggedThread* exception_thread = nullptr;
    for (DebuggedThread* thread : threads) {
      if (thread->koid() == kThreadKoid) {
        exception_thread = thread;
        break;
      }
    }

    ASSERT_TRUE(exception_thread);
    ASSERT_TRUE(exception_thread->in_exception());
    EXPECT_EQ(exception_thread->exception_handle()->GetThreadHandle()->GetKoid(), kThreadKoid);
  }
}

TEST_F(DebugAgentTests, OnEnterLimbo) {
  MockDebugAgentHarness harness;

  constexpr zx_koid_t kProcKoid1 = 100;
  constexpr zx_koid_t kThreadKoid1 = 101;
  harness.system_interface()->mock_limbo_provider().AppendException(
      MockProcessHandle(kProcKoid1, "proc1"), MockThreadHandle(kThreadKoid1, "thread1"),
      MockExceptionHandle(kThreadKoid1));

  // Call the limbo.
  harness.system_interface()->mock_limbo_provider().CallOnEnterLimbo();

  // Should've sent a notification.
  {
    ASSERT_EQ(harness.stream_backend()->process_starts().size(), 1u);

    auto& process_start = harness.stream_backend()->process_starts()[0];
    EXPECT_EQ(process_start.type, debug_ipc::NotifyProcessStarting::Type::kLimbo);
    EXPECT_EQ(process_start.koid, kProcKoid1);
    EXPECT_EQ(process_start.name, "proc1");
  }
}

TEST_F(DebugAgentTests, DetachFromLimbo) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr zx_koid_t kProcKoid = 14;  // MockJobTree job11-p1

  // Attempting to detach to a process that doesn't exist should fail.
  {
    debug_ipc::DetachRequest request = {};
    request.koid = kProcKoid;

    debug_ipc::DetachReply reply = {};
    remote_api->OnDetach(request, &reply);

    ASSERT_TRUE(reply.status.has_error());
    ASSERT_EQ(harness.system_interface()->mock_limbo_provider().release_calls().size(), 0u);
  }

  // Adding it should now find it and remove it.
  constexpr zx_koid_t kProcKoid1 = 100;
  constexpr zx_koid_t kThreadKoid1 = 101;
  harness.system_interface()->mock_limbo_provider().AppendException(
      MockProcessHandle(kProcKoid1, "proc1"), MockThreadHandle(kThreadKoid1, "thread1"),
      MockExceptionHandle(kThreadKoid1));
  {
    debug_ipc::DetachRequest request = {};
    request.koid = kProcKoid1;

    debug_ipc::DetachReply reply = {};
    remote_api->OnDetach(request, &reply);

    ASSERT_TRUE(reply.status.ok());
    ASSERT_EQ(harness.system_interface()->mock_limbo_provider().release_calls().size(), 1u);
    EXPECT_EQ(harness.system_interface()->mock_limbo_provider().release_calls()[0], kProcKoid1);
  }

  // This should've remove it from limbo, trying it again should fail.
  {
    debug_ipc::DetachRequest request = {};
    request.koid = kProcKoid1;

    debug_ipc::DetachReply reply = {};
    remote_api->OnDetach(request, &reply);

    ASSERT_TRUE(reply.status.has_error());
    ASSERT_EQ(harness.system_interface()->mock_limbo_provider().release_calls().size(), 1u);
    EXPECT_EQ(harness.system_interface()->mock_limbo_provider().release_calls()[0], kProcKoid1);
  }
}

TEST_F(DebugAgentTests, Kill) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr zx_koid_t kProcKoid = 14;  // MockJobTree job11-p1

  // Attempt to kill a process that's not there should fail.
  {
    debug_ipc::KillRequest kill_request = {};
    kill_request.process_koid = kProcKoid;

    debug_ipc::KillReply kill_reply = {};
    remote_api->OnKill(kill_request, &kill_reply);
    ASSERT_TRUE(kill_reply.status.has_error());
  }

  // Attach to a process so that the debugger knows about it.
  {
    debug_ipc::AttachRequest attach_request = {};
    attach_request.koid = kProcKoid;

    debug_ipc::AttachReply attach_reply;
    remote_api->OnAttach(attach_request, &attach_reply);

    // There should be a process.
    ASSERT_EQ(harness.debug_agent()->procs_.size(), 1u);
    // Should not come from limbo.
    EXPECT_FALSE(harness.debug_agent()->procs_.begin()->second->from_limbo());
  }

  // Killing now should work.
  {
    debug_ipc::KillRequest kill_request = {};
    kill_request.process_koid = kProcKoid;

    debug_ipc::KillReply kill_reply = {};
    remote_api->OnKill(kill_request, &kill_reply);

    // There should be no more processes.
    ASSERT_EQ(harness.debug_agent()->procs_.size(), 0u);

    // Killing again should fail.
    remote_api->OnKill(kill_request, &kill_reply);
    ASSERT_TRUE(kill_reply.status.has_error());
  }

  // Add the process to the limbo.
  constexpr zx_koid_t kLimboProcKoid = 100;
  constexpr zx_koid_t kLimboThreadKoid = 101;
  MockProcessHandle mock_process(kLimboProcKoid, "proc");
  // This is a limbo process so we can not kill it.
  mock_process.set_kill_status(debug::Status("Access denied"));
  MockThreadHandle mock_thread(kLimboThreadKoid, "thread");
  MockExceptionHandle mock_exception(kLimboThreadKoid);
  mock_process.set_threads({mock_thread});
  harness.system_interface()->mock_limbo_provider().AppendException(mock_process, mock_thread,
                                                                    mock_exception);

  // There should be no more processes.
  ASSERT_EQ(harness.debug_agent()->procs_.size(), 0u);

  // Killing now should release it.
  {
    debug_ipc::KillRequest kill_request = {};
    kill_request.process_koid = kLimboProcKoid;

    debug_ipc::KillReply kill_reply = {};
    remote_api->OnKill(kill_request, &kill_reply);
    ASSERT_TRUE(kill_reply.status.ok());

    ASSERT_EQ(harness.system_interface()->mock_limbo_provider().release_calls().size(), 1u);
    EXPECT_EQ(harness.system_interface()->mock_limbo_provider().release_calls()[0], kLimboProcKoid);

    // Killing again should not find it.
    remote_api->OnKill(kill_request, &kill_reply);
    ASSERT_TRUE(kill_reply.status.has_error());
  }

  harness.system_interface()->mock_limbo_provider().AppendException(mock_process, mock_thread,
                                                                    mock_exception);

  debug_ipc::AttachRequest attach_request = {};
  attach_request.koid = kLimboProcKoid;

  debug_ipc::AttachReply attach_reply;
  remote_api->OnAttach(attach_request, &attach_reply);

  // There should be a process.
  ASSERT_EQ(harness.debug_agent()->procs_.size(), 1u);

  {
    auto it = harness.debug_agent()->procs_.find(kLimboProcKoid);
    ASSERT_NE(it, harness.debug_agent()->procs_.end());
    EXPECT_TRUE(harness.debug_agent()->procs_.begin()->second->from_limbo());

    // Killing it should free the process.
    debug_ipc::KillRequest kill_request = {};
    kill_request.process_koid = kLimboProcKoid;

    debug_ipc::KillReply kill_reply = {};
    remote_api->OnKill(kill_request, &kill_reply);
    ASSERT_TRUE(kill_reply.status.ok());

    ASSERT_EQ(harness.debug_agent()->procs_.size(), 0u);

    // There should be a limbo process to be killed.
    ASSERT_EQ(harness.debug_agent()->killed_limbo_procs_.size(), 1u);
    EXPECT_EQ(harness.debug_agent()->killed_limbo_procs_.count(kLimboProcKoid), 1u);

    // There should've have been more release calls (yet).
    ASSERT_EQ(harness.system_interface()->mock_limbo_provider().release_calls().size(), 1u);

    // When the process "re-enters" the limbo, it should be removed.
    harness.system_interface()->mock_limbo_provider().AppendException(mock_process, mock_thread,
                                                                      mock_exception);
    harness.system_interface()->mock_limbo_provider().CallOnEnterLimbo();

    // There should not be an additional proc in the agent.
    ASSERT_EQ(harness.debug_agent()->procs_.size(), 0u);

    // There should've been a release call.
    ASSERT_EQ(harness.system_interface()->mock_limbo_provider().release_calls().size(), 2u);
    EXPECT_EQ(harness.system_interface()->mock_limbo_provider().release_calls()[1], kLimboProcKoid);
  }
}

TEST_F(DebugAgentTests, OnUpdateGlobalSettings) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  // The default strategy should be first chance for a type that has yet to be
  // updated.
  EXPECT_EQ(debug_ipc::ExceptionStrategy::kFirstChance,
            harness.debug_agent()->GetExceptionStrategy(debug_ipc::ExceptionType::kGeneral));
  EXPECT_EQ(debug_ipc::ExceptionStrategy::kFirstChance,
            harness.debug_agent()->GetExceptionStrategy(debug_ipc::ExceptionType::kPageFault));

  {
    const debug_ipc::UpdateGlobalSettingsRequest request = {
        .exception_strategies =
            {
                {
                    .type = debug_ipc::ExceptionType::kGeneral,
                    .value = debug_ipc::ExceptionStrategy::kSecondChance,
                },
                {
                    .type = debug_ipc::ExceptionType::kPageFault,
                    .value = debug_ipc::ExceptionStrategy::kSecondChance,
                },
            },
    };
    debug_ipc::UpdateGlobalSettingsReply reply;
    remote_api->OnUpdateGlobalSettings(request, &reply);
    EXPECT_TRUE(reply.status.ok());
  }

  EXPECT_EQ(debug_ipc::ExceptionStrategy::kSecondChance,
            harness.debug_agent()->GetExceptionStrategy(debug_ipc::ExceptionType::kGeneral));
  EXPECT_EQ(debug_ipc::ExceptionStrategy::kSecondChance,
            harness.debug_agent()->GetExceptionStrategy(debug_ipc::ExceptionType::kPageFault));

  {
    const debug_ipc::UpdateGlobalSettingsRequest request = {
        .exception_strategies =
            {
                {
                    .type = debug_ipc::ExceptionType::kGeneral,
                    .value = debug_ipc::ExceptionStrategy::kFirstChance,
                },
            },
    };
    debug_ipc::UpdateGlobalSettingsReply reply;
    remote_api->OnUpdateGlobalSettings(request, &reply);
    EXPECT_TRUE(reply.status.ok());
  }

  EXPECT_EQ(debug_ipc::ExceptionStrategy::kFirstChance,
            harness.debug_agent()->GetExceptionStrategy(debug_ipc::ExceptionType::kGeneral));
  EXPECT_EQ(debug_ipc::ExceptionStrategy::kSecondChance,
            harness.debug_agent()->GetExceptionStrategy(debug_ipc::ExceptionType::kPageFault));
}

TEST_F(DebugAgentTests, WeakFilterMatchDoesNotSendModules) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr char kProcessName[] = "process-1";
  constexpr uint64_t kProcessKoid = 0x12345;

  debug_ipc::UpdateFilterRequest request;
  auto& filter = request.filters.emplace_back();
  filter.type = debug_ipc::Filter::Type::kProcessName;
  filter.pattern = kProcessName;
  filter.id = debug_ipc::Filter::Identifier(1, debug_ipc::Filter::Originator::kUnknown);
  filter.config.weak = true;

  debug_ipc::UpdateFilterReply reply;
  remote_api->OnUpdateFilter(request, &reply);

  EXPECT_TRUE(reply.matched_processes_for_filter.empty());

  harness.debug_agent()->OnProcessChanged(
      DebugAgent::ProcessChangedHow::kStarting,
      std::make_unique<MockProcessHandle>(kProcessKoid, kProcessName));

  // We should have sent a process starting notification, but no modules.
  EXPECT_FALSE(harness.stream_backend()->process_starts().empty());
  EXPECT_TRUE(harness.stream_backend()->modules().empty());
}

TEST_F(DebugAgentTests, RecursiveFiltersDoNotCollideUpdateFilter) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  debug_ipc::UpdateFilterRequest request;
  debug_ipc::Filter filter;
  filter.type = debug_ipc::Filter::Type::kComponentMonikerSuffix;
  filter.pattern = "test:test_root";
  filter.id = debug_ipc::Filter::Identifier(1, debug_ipc::Filter::Originator::kUnknown);
  filter.config.recursive = true;
  request.filters.push_back(filter);

  debug_ipc::Filter filter2;
  filter2.type = debug_ipc::Filter::Type::kComponentUrl;
  filter2.pattern = "fuchsia-pkg://devhost/root_package#meta/root_component.cm";
  filter2.id = debug_ipc::Filter::Identifier(2, debug_ipc::Filter::Originator::kUnknown);
  filter2.config.recursive = true;
  request.filters.push_back(filter2);

  debug_ipc::UpdateFilterReply reply;
  remote_api->OnUpdateFilter(request, &reply);

  loop().RunUntilNoTasks();

  // Should have a filter created message.
  EXPECT_EQ(harness.stream_backend()->filters().size(), 3u);

  constexpr char kExpectedMonikerPrefixFilterPattern[] = "/moniker/generated/test:test_root";
  const auto& job4_filter = std::ranges::find_if(
      harness.stream_backend()->filters(),
      [kExpectedMonikerPrefixFilterPattern](const debug_ipc::NotifyFilterCreated& notify) {
        return notify.filter.pattern == kExpectedMonikerPrefixFilterPattern;
      });

  ASSERT_NE(job4_filter, harness.stream_backend()->filters().end());

  // When creating a new filter for matching during UpdateFilter, the client does not have to send
  // another UpdateFilter message to match against the new filter.
  EXPECT_TRUE(job4_filter->participated_in_matching);
  // The first matching filter is the creator of the new filter.
  EXPECT_EQ(job4_filter->originating_filter_id, filter.id);
  EXPECT_EQ(job4_filter->filter.pattern, "/moniker/generated/test:test_root");
  EXPECT_EQ(job4_filter->filter.type, debug_ipc::Filter::Type::kComponentMonikerPrefix);
  EXPECT_EQ(job4_filter->filter.config.recursive, false);
}

TEST_F(DebugAgentTests, RecursiveFiltersDoNotCollideComponentStarting) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr char kFakeMoniker[] = "/moniker/generated/test_some_root";

  debug_ipc::UpdateFilterRequest request;
  debug_ipc::Filter filter;
  filter.type = debug_ipc::Filter::Type::kComponentMonikerSuffix;
  // Intentionally does not match the components in MockSystemInterface.
  filter.pattern = "test_some_root";
  filter.id = debug_ipc::Filter::Identifier(1, debug_ipc::Filter::Originator::kUnknown);
  filter.config.recursive = true;
  request.filters.push_back(filter);

  debug_ipc::Filter filter2;
  filter2.type = debug_ipc::Filter::Type::kComponentUrl;
  // Intentionally does not match the components in MockSystemInterface.
  filter2.pattern = "fuchsia-pkg://devhost/some_package#meta/root_component.cm";
  filter2.id = debug_ipc::Filter::Identifier(2, debug_ipc::Filter::Originator::kUnknown);
  filter2.config.recursive = true;
  request.filters.push_back(filter2);

  debug_ipc::UpdateFilterReply reply;
  remote_api->OnUpdateFilter(request, &reply);

  // Shouldn't match anything yet since these component details are not specified in
  // MockSystemInterface.
  EXPECT_TRUE(reply.matched_processes_for_filter.empty());

  // This event matches both filters created above, but we should only add a single moniker prefix
  // filter to match this.
  harness.system_interface()->mock_component_manager().InjectComponentEvent(
      FakeEventType::kDebugStarted, kFakeMoniker,
      "fuchsia-pkg://devhost/some_package#meta/root_component.cm");

  loop().RunUntilNoTasks();

  // Should have a filter created message.
  EXPECT_EQ(harness.stream_backend()->filters().size(), 1u);
  // The new filter doesn't participate in matching until the client sends it back in an
  // UpdateFilter request.
  EXPECT_FALSE(harness.stream_backend()->filters()[0].participated_in_matching);
  // The first matching filter is the creator of the new filter.
  EXPECT_EQ(harness.stream_backend()->filters()[0].originating_filter_id, filter.id);
  EXPECT_EQ(harness.stream_backend()->filters()[0].filter.pattern, kFakeMoniker);
  EXPECT_EQ(harness.stream_backend()->filters()[0].filter.type,
            debug_ipc::Filter::Type::kComponentMonikerPrefix);
  EXPECT_EQ(harness.stream_backend()->filters()[0].filter.config.recursive, false);
}

TEST_F(DebugAgentTests, RecursiveFilterAppliesImplicitFilter) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr char kRootComponentUrl[] = "fuchsia-pkg://devhost/root_package#meta/root_component.cm";
  constexpr char kRootComponentMoniker[] = "/moniker/generated/test:test_root";
  // From MockSystemInterface.
  constexpr zx_koid_t kJob5P1Koid = 33;

  debug_ipc::UpdateFilterRequest request;
  auto& filter = request.filters.emplace_back();
  filter.type = debug_ipc::Filter::Type::kComponentUrl;
  filter.pattern = kRootComponentUrl;
  filter.config.recursive = true;

  debug_ipc::UpdateFilterReply reply;
  remote_api->OnUpdateFilter(request, &reply);

  // We should have received a filter created notification.
  ASSERT_FALSE(harness.stream_backend()->filters().empty());
  const auto& filters = harness.stream_backend()->filters();
  ASSERT_EQ(filters.size(), 1u);
  EXPECT_EQ(filters[0].originating_filter_id, request.filters[0].id);
  EXPECT_EQ(filters[0].filter.id.Decode().originator, debug_ipc::Filter::Originator::kAgent);
  EXPECT_EQ(filters[0].filter.pattern, kRootComponentMoniker);
  EXPECT_EQ(filters[0].filter.type, debug_ipc::Filter::Type::kComponentMonikerPrefix);
  EXPECT_FALSE(filters[0].filter.config.recursive);
  // Matching for the new filter happens along with the rest of the new filters during the
  // UpdateFilter request.
  EXPECT_TRUE(filters[0].participated_in_matching);

  // The root component that matches the filter in our |UpdateFilterRequest| doesn't have a process
  // associated with it, but we should match the process in that component's realm in the
  // subpackaged component.
  ASSERT_EQ(reply.matched_processes_for_filter.size(), 1u);
  EXPECT_EQ(reply.matched_processes_for_filter[0].id, filters[0].filter.id);
  EXPECT_EQ(reply.matched_processes_for_filter[0].matches.size(), 1u);
  EXPECT_EQ(reply.matched_processes_for_filter[0].matches[0].koid, kJob5P1Koid);
  EXPECT_EQ(reply.matched_processes_for_filter[0].matches[0].type, debug_ipc::TaskType::kProcess);
}

TEST_F(DebugAgentTests, AttachToExistingJob) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr zx_koid_t kJobKoid = 8;
  constexpr zx_koid_t kProcessKoid = 9;

  debug_ipc::AttachRequest request;
  request.koid = kJobKoid;
  request.config.target = debug_ipc::TaskType::kJob;
  request.config.priority = debug_ipc::AttachConfig::Priority::kWeak;

  debug_ipc::AttachReply reply;
  remote_api->OnAttach(request, &reply);

  // Because we didn't inject all of the notifications.
  auto debugged_job = harness.debug_agent()->GetDebuggedJob(kJobKoid);
  ASSERT_TRUE(debugged_job);

  // Simply attaching to a job won't necessarily create the DebuggedProcess objects (i.e. the client
  // issued a direct attach command to an already running component instead of installing a filter
  // and waiting for a matching component to start).
  EXPECT_FALSE(harness.debug_agent()->GetDebuggedProcess(kProcessKoid));
}

TEST_F(DebugAgentTests, JobOnlyFilter) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  // Matches the default job tree in MockSystemInterface.
  constexpr zx_koid_t kJobKoid = 25;
  constexpr zx_koid_t kProcessKoid = 26;
  constexpr char kComponentRootMoniker[] = "fixed/moniker";

  debug_ipc::UpdateFilterRequest request;
  auto& filter = request.filters.emplace_back();
  filter.type = debug_ipc::Filter::Type::kComponentMonikerSuffix;
  filter.pattern = kComponentRootMoniker;
  filter.config.job_only = true;
  // By specifying a strong attach, we'll claim the job's exception channel, not the debugger
  // exception channel.
  filter.config.weak = false;

  debug_ipc::UpdateFilterReply reply;
  remote_api->OnUpdateFilter(request, &reply);

  // Should have matched the existing component.
  EXPECT_EQ(reply.matched_processes_for_filter.size(), 1u);
  EXPECT_EQ(reply.matched_processes_for_filter[0].id, filter.id);
  EXPECT_EQ(reply.matched_processes_for_filter[0].matches.size(), 1u);
  EXPECT_EQ(reply.matched_processes_for_filter[0].matches[0].koid, kJobKoid);
  EXPECT_EQ(reply.matched_processes_for_filter[0].matches[0].type, debug_ipc::TaskType::kJob);

  debug_ipc::AttachRequest attach_request;
  attach_request.koid = reply.matched_processes_for_filter[0].matches[0].koid;
  attach_request.config = debug_ipc::FilterConfig::ToAttachConfig(filter.config);
  debug_ipc::AttachReply attach_reply;

  remote_api->OnAttach(attach_request, &attach_reply);
  EXPECT_TRUE(attach_reply.status.ok()) << attach_reply.status.message();

  // The job_only filter will cause the DebuggedProcess object to be created and propagate the
  // process starting notification.
  harness.debug_agent()->OnProcessChanged(
      DebugAgent::ProcessChangedHow::kStarting,
      harness.debug_agent()->system_interface().GetProcess(kProcessKoid));

  // We should now have both a DebuggedJob and a DebuggedProcess for this job and process. The
  // process should *not* have the exception channel bound, because the filter was configured as
  // job_only.
  EXPECT_EQ(harness.stream_backend()->process_starts().size(), 1u);
  EXPECT_TRUE(harness.debug_agent()->GetDebuggedJob(kJobKoid));

  // We should have a DebuggedProcess object, but should not be attached to its exception channel.
  ASSERT_TRUE(harness.debug_agent()->GetDebuggedProcess(kProcessKoid));
  EXPECT_FALSE(harness.debug_agent()->GetDebuggedProcess(kProcessKoid)->IsAttached());
}

TEST_F(DebugAgentTests, JobOnlyFilterDoesNotAttachToChildJobs) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  // Matches the default job tree in MockSystemInterface. This is "job1".
  constexpr zx_koid_t kJobKoid = 8;
  // Direct child process of "job1".
  constexpr zx_koid_t kProcessKoid = 9;
  // This is a child job of the above, "job11".
  constexpr zx_koid_t kSecondJobKoid = 13;
  // Child of job11.
  constexpr zx_koid_t kSecondProcessKoid = 14;

  // Component moniker associated with all above jobs.
  constexpr char kComponentRootMoniker[] = "/moniker";

  debug_ipc::UpdateFilterRequest request;
  auto& filter = request.filters.emplace_back();
  filter.type = debug_ipc::Filter::Type::kComponentMonikerSuffix;
  filter.pattern = kComponentRootMoniker;
  filter.config.job_only = true;
  // By specifying a strong attach, we'll claim the job's exception channel, not the debugger
  // exception channel.
  filter.config.weak = false;

  debug_ipc::UpdateFilterReply reply;
  // We don't need to worry about the contents of the reply, it will have all of the child processes
  // under the component. In a real system, the client will choose what to attach to.
  remote_api->OnUpdateFilter(request, &reply);

  // Attach to the root job of the component from the component starting event.
  harness.debug_agent()->OnComponentStarted(kComponentRootMoniker, "some/url", kJobKoid);

  ASSERT_EQ(harness.stream_backend()->component_starts().size(), 1u);

  auto component_start = harness.stream_backend()->component_starts()[0];
  ASSERT_EQ(component_start.matching_filters.size(), 1u);

  auto attach_configs = debug_ipc::GetAttachConfigsForFilterMatches(
      component_start.matching_filters, harness.debug_agent()->GetIpcFilters());
  ASSERT_EQ(attach_configs.size(), 1u);
  ASSERT_TRUE(attach_configs.contains(kJobKoid));

  const auto& attach_config = attach_configs.find(kJobKoid)->second;
  EXPECT_EQ(attach_config.priority, debug_ipc::AttachConfig::Priority::kStrong);
  EXPECT_EQ(attach_config.target, debug_ipc::TaskType::kJob);

  debug_ipc::AttachRequest attach_request;
  attach_request.koid = kJobKoid;
  attach_request.config = attach_config;

  debug_ipc::AttachReply attach_reply;
  harness.debug_agent()->OnAttach(attach_request, &attach_reply);

  ASSERT_TRUE(attach_reply.status.ok());

  // Send the notification that the child process of "job1" started. This one will always start
  // first, and will create the DebuggedProcess objects.
  harness.debug_agent()->OnProcessChanged(
      DebugAgent::ProcessChangedHow::kStarting,
      harness.debug_agent()->system_interface().GetProcess(kProcessKoid));

  // Send the notification that the child process of "job11" started. This should _not_ attach to
  harness.debug_agent()->OnProcessChanged(
      DebugAgent::ProcessChangedHow::kStarting,
      harness.debug_agent()->system_interface().GetProcess(kSecondProcessKoid));

  // We should now have both processes specified above. Neither process should have the exception
  // channel bound, because the filter was configured as job_only. Furthermore, the parent job
  // _should_ be attached (i.e. we'll have a DebuggedJob object for it) and the child job should
  // _not_ be attached.
  EXPECT_EQ(harness.stream_backend()->process_starts().size(), 2u);
  EXPECT_TRUE(harness.debug_agent()->GetDebuggedJob(kJobKoid));
  // We are _not_ attached to this second job, even though it would also match the component filter.
  EXPECT_FALSE(harness.debug_agent()->GetDebuggedJob(kSecondJobKoid));
}

// This test is very similar to the above test. The difference is in the component structure. This
// test's filter matches multiple components, which respectively in a real system would have unique
// job_ids, and therefore return multiple results in the filter matching logic. This is different
// from the above situation where one component (and its corresponding job) contains multiple jobs
// but no child components.
TEST_F(DebugAgentTests, DoNotAttachToChildJobs) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  // From the default MockSystemInterface.
  constexpr zx_koid_t kParentJobKoid = 35;
  constexpr zx_koid_t kChildJobKoid = 38;
  constexpr char kRootComponentMoniker[] = "/some/moniker";
  constexpr char kChildComponentMoniker[] = "/some/other/moniker";

  // Start the components before the filter. The filter should match immediately in |OnUpdateFilter|
  // below.
  harness.debug_agent()->OnComponentStarted(kRootComponentMoniker, "some/url", kParentJobKoid);
  harness.debug_agent()->OnComponentStarted(kChildComponentMoniker, "some/url", kParentJobKoid);

  // This realm has two ELF components, one is the realm's root, the other is a child. They each
  // have distinctive job_ids.
  constexpr char kMonikerPrefix[] = "/some";

  debug_ipc::UpdateFilterRequest request;
  auto& filter = request.filters.emplace_back();
  filter.type = debug_ipc::Filter::Type::kComponentMonikerPrefix;
  filter.pattern = kMonikerPrefix;
  filter.config.job_only = true;
  // By specifying a strong attach, we'll claim the job's exception channel, not the debugger
  // exception channel.
  filter.config.weak = false;

  debug_ipc::UpdateFilterReply reply;
  remote_api->OnUpdateFilter(request, &reply);

  // We should have 1 filter match and two job koids.
  EXPECT_FALSE(reply.matched_processes_for_filter.empty());
  EXPECT_EQ(reply.matched_processes_for_filter.size(), 1u);
  EXPECT_EQ(reply.matched_processes_for_filter[0].matches.size(), 2u);

  auto configs = debug_ipc::GetAttachConfigsForFilterMatches(
      reply.matched_processes_for_filter, harness.debug_agent()->GetIpcFilters());

  // We will attempt to attach to both.
  EXPECT_EQ(configs.size(), 2u);

  for (const auto& [koid, config] : configs) {
    debug_ipc::AttachRequest request;
    request.koid = koid;
    request.config = config;

    debug_ipc::AttachReply reply;
    remote_api->OnAttach(request, &reply);

    if (koid == kParentJobKoid) {
      EXPECT_TRUE(reply.status.ok());
    } else {
      // We should get an already exists error for the child job.
      EXPECT_TRUE(reply.status.has_error());
      EXPECT_EQ(reply.status.type(), debug::Status::Type::kAlreadyExists);
    }
  }

  EXPECT_TRUE(harness.debug_agent()->GetDebuggedJob(kParentJobKoid));
  EXPECT_FALSE(harness.debug_agent()->GetDebuggedJob(kChildJobKoid));
}

TEST_F(DebugAgentTests, RecursiveJobOnlyFilterDoesNotCannibalizeChildComponents) {
  constexpr zx_koid_t kRootJobKoid = 1001;
  constexpr zx_koid_t kParentJobKoid = 1002;
  constexpr zx_koid_t kParentJobProcessKoid = 1003;
  constexpr zx_koid_t kChildJobKoid = 1005;
  constexpr zx_koid_t kChildJobProcessKoid = 1006;

  // We're going to create a custom component topology on the fly for this test. Here is what the
  // initial state looks like:
  //
  // j: 1001 root
  //   j: 1002 parent-job
  //     p: 1003 parent-job-p1 some/moniker fuchsia-pkg://some/url
  //       t: 1004 initial-thread
  MockProcessHandle parent_job_process(kParentJobProcessKoid, "parent-job-p1");
  parent_job_process.set_threads({MockThreadHandle(1004, "initial-thread")});
  MockJobHandle parent_component_job(kParentJobKoid, "parent-job");
  parent_component_job.set_child_processes({std::move(parent_job_process)});

  MockJobHandle root_job(kRootJobKoid, "root");
  root_job.set_child_jobs({std::move(parent_component_job)});

  auto unique_mock_system_interface = std::make_unique<MockSystemInterface>(std::move(root_job));
  auto mock_system_interface = unique_mock_system_interface.get();

  constexpr char kParentComponentMoniker[] = "some/moniker";
  constexpr char kChildComponentMoniker[] = "some/moniker/other/moniker";

  mock_system_interface->mock_component_manager().AddComponentInfo(
      kParentJobKoid, {.moniker = kParentComponentMoniker, .url = "fuchsia-pkg://some/url"});

  MockDebugAgentHarness harness(std::move(unique_mock_system_interface));
  RemoteAPI* remote_api = harness.debug_agent();

  // Install a recursive job-only filter. This will install its recursive (process-only) counterpart
  // immediately upon installation since it should already match.
  debug_ipc::UpdateFilterRequest request;
  auto& filter = request.filters.emplace_back();
  filter.type = debug_ipc::Filter::Type::kComponentMonikerSuffix;
  filter.pattern = "moniker";
  filter.config.job_only = true;
  filter.config.recursive = true;

  // This will immediately match the parent component and install the recursive filter.
  debug_ipc::UpdateFilterReply reply;
  remote_api->OnUpdateFilter(request, &reply);

  // We should have gotten a filter created event.
  ASSERT_EQ(harness.stream_backend()->filters().size(), 1u);
  EXPECT_EQ(harness.stream_backend()->filters()[0].originating_filter_id, filter.id);

  const auto& moniker_prefix_filter = harness.stream_backend()->filters()[0].filter;
  EXPECT_EQ(moniker_prefix_filter.pattern, kParentComponentMoniker);
  EXPECT_FALSE(moniker_prefix_filter.config.job_only);
  EXPECT_FALSE(moniker_prefix_filter.config.recursive);
  EXPECT_TRUE(moniker_prefix_filter.config.never_attach);

  // Now, we want to inject a new child component that matches both the original recursive job-only
  // filter and the new moniker prefix filter.
  MockProcessHandle child_job_process(kChildJobProcessKoid, "child-job-p1");
  child_job_process.set_threads({MockThreadHandle(1007, "initial-thread")});
  MockJobHandle child_job(kChildJobKoid, "child-job");
  child_job.set_child_processes({std::move(child_job_process)});

  debug_ipc::ComponentInfo child_info = {.moniker = kChildComponentMoniker,
                                         .url = "fuchsia-pkg://some/other/url"};

  // The job is now inserted into the job tree along with the component info, so the entire tree
  // looks like this now:
  //
  // j: 1001 root
  //   j: 1002 parent-job some/moniker fuchsia-pkg://some/url
  //     p: 1003 parent-job-p1
  //       t: 1004 initial-thread
  //     j: 1005 child-job some/moniker/other/moniker fuchsia-pkg://some/other/url
  //       p: 1006 child-job-p1
  //         t: 1007 initial-thread
  mock_system_interface->AddJob(std::move(child_job), kParentJobKoid, child_info);

  // Inject the component starting event, so that our filters notice.
  harness.debug_agent()->OnComponentStarted(child_info.moniker, child_info.url, kChildJobKoid);

  // We should have received the event. This will be for the job, which we don't want to issue an
  // attach for (the DebugAgent would ignore it anyway since we're attached to the parent).
  EXPECT_EQ(harness.stream_backend()->component_starts().size(), 1u);

  // We should still only have the one extra filter added from before, there should *not* be a new
  // recursive filter installed for the child's complete moniker (which is redundant since the
  // parent's moniker will match everything in the child component).
  ASSERT_EQ(harness.stream_backend()->filters().size(), 1u);
  EXPECT_EQ(harness.stream_backend()->filters()[0].originating_filter_id, filter.id);
  EXPECT_EQ(harness.stream_backend()->filters()[0].filter.pattern, kParentComponentMoniker);

  // Send the notification that the child process of "child-job" started.
  harness.debug_agent()->OnProcessChanged(
      DebugAgent::ProcessChangedHow::kStarting,
      harness.debug_agent()->system_interface().GetProcess(kChildJobProcessKoid));

  EXPECT_EQ(harness.stream_backend()->process_starts().size(), 1u);
  // Both of our filters should be reported as a match. The order is not guaranteed.
  ASSERT_EQ(harness.stream_backend()->process_starts()[0].filter_ids.size(), 2u);
  auto found = std::ranges::find(harness.stream_backend()->process_starts()[0].filter_ids,
                                 moniker_prefix_filter.id);
  EXPECT_NE(found, harness.stream_backend()->process_starts()[0].filter_ids.end());
  found = std::ranges::find(harness.stream_backend()->process_starts()[0].filter_ids, filter.id);
  EXPECT_NE(found, harness.stream_backend()->process_starts()[0].filter_ids.end());

  // And we should still not have any other filter created events.
  ASSERT_EQ(harness.stream_backend()->filters().size(), 1u);
  EXPECT_EQ(harness.stream_backend()->filters()[0].originating_filter_id, filter.id);
  EXPECT_EQ(harness.stream_backend()->filters()[0].filter.pattern, kParentComponentMoniker);

  // We should be explicitly attached to the child job's process, since it was created after we had
  // a matching filter in place. Note that since we didn't issue explicit attach requests in
  // response to the UpdateFilter request at the beginning of this test, we are _not_ attached to
  // the parent job or its process.
  EXPECT_TRUE(harness.debug_agent()->GetDebuggedProcess(kChildJobProcessKoid));
  // We are _not_ attached to this second job, even though it would also match the component moniker
  // prefix filter, because that filter is not configured as job-only.
  EXPECT_FALSE(harness.debug_agent()->GetDebuggedJob(kChildJobKoid));

  // Check that we don't have any extra filters installed other than the original recursive job
  // filter and the highest level component moniker prefix filter.
  debug_ipc::StatusReply status;
  remote_api->OnStatus({}, &status);
  ASSERT_EQ(status.filters.size(), 2u);
  EXPECT_FALSE(status.filters[0].config.job_only);
  EXPECT_FALSE(status.filters[0].config.recursive);
  EXPECT_EQ(status.filters[0].pattern, kParentComponentMoniker);
  EXPECT_EQ(status.filters[0].type, debug_ipc::Filter::Type::kComponentMonikerPrefix);
  EXPECT_TRUE(status.filters[1].config.job_only);
  EXPECT_TRUE(status.filters[1].config.recursive);
  EXPECT_EQ(status.filters[1].pattern, "moniker");
  EXPECT_EQ(status.filters[1].type, debug_ipc::Filter::Type::kComponentMonikerSuffix);
}

TEST_F(DebugAgentTests, OnResumeDoesNotSuspendNewThreads) {
  MockDebugAgentHarness harness;

  constexpr uint64_t kProcessKoid = 1233;
  constexpr uint64_t kThread1Koid = 1234;
  constexpr uint64_t kThread2Koid = 1235;
  constexpr uint64_t kThread3Koid = 1236;

  auto* mock_process = harness.AddProcess(kProcessKoid);
  mock_process->AddThread(kThread1Koid);

  ASSERT_TRUE(mock_process);
  ASSERT_EQ(mock_process->GetThreads().size(), 1u);

  std::ignore = mock_process->ClientSuspendAllThreads();

  auto* debugged_thread = mock_process->GetThread(kThread1Koid);
  ASSERT_TRUE(debugged_thread);
  EXPECT_TRUE(debugged_thread->is_client_suspended());

  // Inject the thread starting notification. The thread should now be suspended. We have to inject
  // the new thread into the MockProcessHandle so DebuggedProcess can re-populate its threads to
  // find the new one.
  MockThreadHandle thread2_handle(kThread2Koid);
  mock_process->mock_process_handle().AddThreadAndSendEvent(std::move(thread2_handle));

  auto* debugged_thread2 = mock_process->GetThread(kThread2Koid);
  ASSERT_TRUE(debugged_thread2);
  EXPECT_TRUE(debugged_thread2->is_client_suspended());

  // Issue the resume as separate ProcessThreadIds, which simulates how zxdb does a "Process
  // Continue".
  debug_ipc::ResumeRequest resume_request;
  resume_request.how = debug_ipc::ResumeRequest::How::kResolveAndContinue;
  resume_request.ids = {
      {.process = kProcessKoid, .thread = kThread1Koid},
      {.process = kProcessKoid, .thread = kThread2Koid},
  };
  debug_ipc::ResumeReply reply;
  harness.debug_agent()->OnResume(resume_request, &reply);

  EXPECT_FALSE(debugged_thread->is_client_suspended());
  EXPECT_FALSE(debugged_thread2->is_client_suspended());

  MockThreadHandle thread3_handle(kThread3Koid);
  mock_process->mock_process_handle().AddThreadAndSendEvent(std::move(thread3_handle));
  auto* thread3 = mock_process->GetThread(kThread3Koid);
  ASSERT_TRUE(thread3);
  EXPECT_FALSE(thread3->is_client_suspended());
}

TEST_F(DebugAgentTests, OnThreadsUpdatesThreadsForProcess) {
  MockDebugAgentHarness harness;
  RemoteAPI* remote_api = harness.debug_agent();

  constexpr uint64_t kProcessKoid = 9876;
  constexpr uint64_t kThread1Koid = 9877;
  constexpr uint64_t kThread2Koid = 9878;
  constexpr uint64_t kThread3Koid = 9879;

  MockProcessHandle* mock_process_handle_ptr;
  {
    // Manually setup a mock process and thread handle so we can
    MockThreadHandle initial_thread(kThread1Koid);

    auto mock_process_handle = std::make_unique<MockProcessHandle>(kProcessKoid);
    mock_process_handle->set_threads({std::move(initial_thread)});

    DebuggedProcessCreateInfo create_info(std::move(mock_process_handle));

    auto mock_process =
        std::make_unique<MockProcess>(harness.debug_agent(), std::move(create_info));

    mock_process_handle_ptr = reinterpret_cast<MockProcessHandle*>(&mock_process->process_handle());

    harness.debug_agent()->InjectProcessForTest(std::move(mock_process));
  }

  // Now we have the pointer to the mock source of truth in the thread handle. We're going to update
  // the MockProcessHandle's thread list WITHOUT sending notifications to the DebuggedProcess
  // object. This situation can happen when we're recursively weakly attached to a job and enumerate
  // processes, but do not receive thread starting exceptions and are required to manually
  // synchronize with the underlying source of truth.

  {
    // Inject the new state of the process to have additional threads that have stared since we last
    // synchronized.
    MockThreadHandle initial_thread(kThread1Koid);
    MockThreadHandle thread2(kThread2Koid);
    MockThreadHandle thread3(kThread3Koid);

    mock_process_handle_ptr->set_threads(
        {std::move(initial_thread), std::move(thread2), std::move(thread3)});
  }

  // Now send a ThreadsRequest, which should send back the synchronized list of threads.
  debug_ipc::ThreadsRequest request;
  request.process_koid = kProcessKoid;
  debug_ipc::ThreadsReply reply;
  remote_api->OnThreads(request, &reply);

  ASSERT_EQ(reply.threads.size(), 3u);
  EXPECT_EQ(reply.threads[0].id.process, kProcessKoid);
  EXPECT_EQ(reply.threads[0].id.thread, kThread1Koid);
  EXPECT_EQ(reply.threads[1].id.process, kProcessKoid);
  EXPECT_EQ(reply.threads[1].id.thread, kThread2Koid);
  EXPECT_EQ(reply.threads[2].id.process, kProcessKoid);
  EXPECT_EQ(reply.threads[2].id.thread, kThread3Koid);
}

TEST_F(DebugAgentTests, MonitorRootJobOption) {
  {
    DebugAgentOptions options;
    options.monitor_root_job = false;
    DebugAgent agent(MockSystemInterface::CreateWithData(), options);
    ASSERT_TRUE(agent.AttachToRootJob().ok());
    auto root_koid = agent.system_interface().GetRootJob()->GetKoid();
    DebuggedJob* job = agent.GetDebuggedJob(root_koid);
    ASSERT_NE(job, nullptr);
    EXPECT_EQ(job->type(), JobExceptionChannelType::kNone);
  }
  {
    DebugAgentOptions options;
    options.monitor_root_job = true;
    DebugAgent agent(MockSystemInterface::CreateWithData(), options);
    ASSERT_TRUE(agent.AttachToRootJob().ok());
    auto root_koid = agent.system_interface().GetRootJob()->GetKoid();
    DebuggedJob* job = agent.GetDebuggedJob(root_koid);
    ASSERT_NE(job, nullptr);
    EXPECT_EQ(job->type(), JobExceptionChannelType::kDebugger);
  }
}
}  // namespace debug_agent
