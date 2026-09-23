// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/developer/debug/debug_agent/debugged_process.h"

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/arch.h"
#include "src/developer/debug/debug_agent/breakpoint.h"
#include "src/developer/debug/debug_agent/debug_agent.h"
#include "src/developer/debug/debug_agent/hardware_breakpoint.h"
#include "src/developer/debug/debug_agent/mock_debug_agent_harness.h"
#include "src/developer/debug/debug_agent/mock_exception_handle.h"
#include "src/developer/debug/debug_agent/mock_process.h"
#include "src/developer/debug/debug_agent/mock_thread.h"
#include "src/developer/debug/debug_agent/mock_thread_handle.h"
#include "src/developer/debug/debug_agent/software_breakpoint.h"
#include "src/developer/debug/debug_agent/test_utils.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"

namespace debug_agent {
namespace {

class MockProcessDelegate : public Breakpoint::ProcessDelegate {
 public:
  debug::Status RegisterBreakpoint(Breakpoint*, zx_koid_t, uint64_t) override {
    return debug::Status();
  }
  void UnregisterBreakpoint(Breakpoint* bp, zx_koid_t process_koid, uint64_t address) override {}

  debug::Status RegisterWatchpoint(Breakpoint*, zx_koid_t, const debug::AddressRange&) override {
    return debug::Status();
  }
  void UnregisterWatchpoint(Breakpoint*, zx_koid_t, const debug::AddressRange&) override {}
};

// Fills a vector with the breakpoint instruction for the current architecture. The size will vary
// by architecture.
std::vector<uint8_t> GetBreakpointMemory() {
  std::vector<uint8_t> result;
  result.resize(arch::kBreakInstructionSize);
  memcpy(result.data(), &arch::kBreakInstruction, arch::kBreakInstructionSize);
  return result;
}

// If |thread| is null, it means a process-wide breakpoint.
debug_ipc::ProcessBreakpointSettings CreateLocation(zx_koid_t process_koid, zx_koid_t thread_koid,
                                                    uint64_t address) {
  debug_ipc::ProcessBreakpointSettings location = {};
  location.id = {.process = process_koid, .thread = thread_koid};
  location.address = address;

  return location;
}

debug_ipc::ProcessBreakpointSettings CreateLocation(zx_koid_t process_koid, zx_koid_t thread_koid,
                                                    const debug::AddressRange& range) {
  debug_ipc::ProcessBreakpointSettings location = {};
  location.id = {.process = process_koid, .thread = thread_koid};
  location.address_range = range;

  return location;
}

debug::AddressRange SetLocation(Breakpoint* breakpoint, zx_koid_t koid,
                                const debug::AddressRange& range) {
  debug_ipc::BreakpointSettings settings;
  settings.type = debug_ipc::BreakpointType::kWrite;
  settings.locations.push_back(CreateLocation(koid, 0, range));
  breakpoint->SetSettings(settings);

  return range;
}

// Tests -------------------------------------------------------------------------------------------

constexpr zx_koid_t kProcessKoid = 0x1;
const std::string kProcessName = "process-name";
constexpr uint64_t kAddress1 = 0x1234;
constexpr uint64_t kAddress2 = 0x5678;
constexpr uint64_t kAddress3 = 0x9abc;
constexpr uint64_t kAddress4 = 0xdef0;

constexpr debug::AddressRange kAddressRange1 = {0x1, 0x2};

TEST(DebuggedProcess, RegisterBreakpoints) {
  MockProcessDelegate process_delegate;
  MockProcess process(nullptr, kProcessKoid, kProcessName);

  // There needs to be memory backing the breakpoints so set some memory here. When the breakpoint
  // is uninstalled, the breakpoint expects that it will be the debug break instruction. So we
  // handle both cases and always report a breakpoint at these addresses.
  process.mock_process_handle().mock_memory().AddMemory(kAddress1, GetBreakpointMemory());
  process.mock_process_handle().mock_memory().AddMemory(kAddress2, GetBreakpointMemory());
  process.mock_process_handle().mock_memory().AddMemory(kAddress3, GetBreakpointMemory());
  process.mock_process_handle().mock_memory().AddMemory(kAddress4, GetBreakpointMemory());

  debug_ipc::BreakpointSettings settings;
  settings.type = debug_ipc::BreakpointType::kSoftware;
  settings.locations.push_back(CreateLocation(kProcessKoid, 0, kAddress1));
  settings.locations.push_back(CreateLocation(kProcessKoid, 0, kAddress2));
  settings.locations.push_back(CreateLocation(kProcessKoid, 0, kAddress3));
  Breakpoint breakpoint(&process_delegate);
  breakpoint.SetSettings(settings);

  ASSERT_TRUE(process.RegisterBreakpoint(&breakpoint, kAddress1).ok());

  ASSERT_EQ(process.software_breakpoints().size(), 1u);
  auto it = process.software_breakpoints().begin();
  EXPECT_EQ(it++->first, kAddress1);
  EXPECT_EQ(it, process.software_breakpoints().end());

  // Add 2 other breakpoints.
  ASSERT_TRUE(process.RegisterBreakpoint(&breakpoint, kAddress2).ok());
  ASSERT_TRUE(process.RegisterBreakpoint(&breakpoint, kAddress3).ok());
  ASSERT_EQ(process.software_breakpoints().size(), 3u);
  it = process.software_breakpoints().begin();
  EXPECT_EQ(it++->first, kAddress1);
  EXPECT_EQ(it++->first, kAddress2);
  EXPECT_EQ(it++->first, kAddress3);
  EXPECT_EQ(it, process.software_breakpoints().end());

  // Unregister a breakpoint.
  process.UnregisterBreakpoint(&breakpoint, kAddress1);
  ASSERT_EQ(process.software_breakpoints().size(), 2u);
  it = process.software_breakpoints().begin();
  EXPECT_EQ(it++->first, kAddress2);
  EXPECT_EQ(it++->first, kAddress3);
  EXPECT_EQ(it, process.software_breakpoints().end());

  // Register a hardware breakpoint.
  Breakpoint hw_breakpoint(&process_delegate);
  debug_ipc::BreakpointSettings hw_settings;
  hw_settings.type = debug_ipc::BreakpointType::kHardware;
  hw_settings.locations.push_back(CreateLocation(kProcessKoid, 0, kAddress4));
  hw_breakpoint.SetSettings(hw_settings);

  ASSERT_TRUE(process.RegisterBreakpoint(&hw_breakpoint, kAddress3).ok());
  ASSERT_TRUE(process.RegisterBreakpoint(&hw_breakpoint, kAddress4).ok());

  // Should've inserted 2 HW breakpoint.
  ASSERT_EQ(process.software_breakpoints().size(), 2u);
  ASSERT_EQ(process.hardware_breakpoints().size(), 2u);
  auto hw_it = process.hardware_breakpoints().begin();
  EXPECT_EQ(hw_it++->first, kAddress3);
  EXPECT_EQ(hw_it++->first, kAddress4);
  EXPECT_EQ(hw_it, process.hardware_breakpoints().end());

  // Remove a hardware breakpoint.
  process.UnregisterBreakpoint(&hw_breakpoint, kAddress3);
  ASSERT_EQ(process.software_breakpoints().size(), 2u);
  ASSERT_EQ(process.hardware_breakpoints().size(), 1u);
  hw_it = process.hardware_breakpoints().begin();
  EXPECT_EQ(hw_it++->first, kAddress4);
  EXPECT_EQ(hw_it, process.hardware_breakpoints().end());

  // Add a watchpoint.
  Breakpoint wp_breakpoint(&process_delegate);
  debug_ipc::BreakpointSettings wp_settings;
  wp_settings.type = debug_ipc::BreakpointType::kWrite;
  wp_settings.locations.push_back(CreateLocation(kProcessKoid, 0, kAddressRange1));
  wp_breakpoint.SetSettings(wp_settings);

  ASSERT_TRUE(process.RegisterWatchpoint(&wp_breakpoint, kAddressRange1).ok());
  ASSERT_EQ(process.software_breakpoints().size(), 2u);
  ASSERT_EQ(process.hardware_breakpoints().size(), 1u);
  ASSERT_EQ(process.watchpoints().size(), 1u);
  auto wp_it = process.watchpoints().begin();
  EXPECT_EQ(wp_it++->first, kAddressRange1);
  EXPECT_EQ(wp_it, process.watchpoints().end());
}

TEST(DebuggedProcess, WatchpointRegistration) {
  MockProcessDelegate process_delegate;
  Breakpoint breakpoint(&process_delegate);

  MockProcess process(nullptr, kProcessKoid, kProcessName);

  // 1 byte.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 1};
    SetLocation(&breakpoint, process.koid(), range);
    ASSERT_TRUE(process.RegisterWatchpoint(&breakpoint, range).ok());
  }

  // 2 bytes.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 2};
    SetLocation(&breakpoint, process.koid(), range);

    // Only aligned values should work.
    bool expected_ok = (range.begin() & 0b1) == 0;
    SCOPED_TRACE(range.ToString());
    ASSERT_EQ(process.RegisterWatchpoint(&breakpoint, range).ok(), expected_ok);
  }

  // 3 bytes.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 3};
    SetLocation(&breakpoint, process.koid(), range);
    ASSERT_TRUE(process.RegisterWatchpoint(&breakpoint, range).has_error());
  }

  // 4 bytes.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 4};
    SetLocation(&breakpoint, process.koid(), range);

    // Only aligned values should work.
    bool expected_ok = (range.begin() & 0b11) == 0;
    SCOPED_TRACE(range.ToString());
    ASSERT_EQ(process.RegisterWatchpoint(&breakpoint, range).ok(), expected_ok);
  }

  // 5 bytes.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 5};
    SetLocation(&breakpoint, process.koid(), range);
    ASSERT_TRUE(process.RegisterWatchpoint(&breakpoint, range).has_error());
  }

  // 6 bytes.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 6};
    SetLocation(&breakpoint, process.koid(), range);
    ASSERT_TRUE(process.RegisterWatchpoint(&breakpoint, range).has_error());
  }

  // 6 bytes.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 6};
    SetLocation(&breakpoint, process.koid(), range);
    ASSERT_TRUE(process.RegisterWatchpoint(&breakpoint, range).has_error());
  }

  // 7 bytes.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 7};
    SetLocation(&breakpoint, process.koid(), range);
    ASSERT_TRUE(process.RegisterWatchpoint(&breakpoint, range).has_error());
  }

  // 8 bytes.
  for (uint32_t i = 0; i < 16; i++) {
    debug::AddressRange range = {0x10 + i, 0x10 + i + 8};
    SetLocation(&breakpoint, process.koid(), range);

    // Only aligned values should work.
    bool expected_ok = (range.begin() & 0b111) == 0;
    SCOPED_TRACE(range.ToString());
    ASSERT_EQ(process.RegisterWatchpoint(&breakpoint, range).ok(), expected_ok);
  }
}

TEST(DebuggedProcess, DetachFromProcess) {
  MockProcess process(nullptr, kProcessKoid, kProcessName);
  MockThread* thread = process.AddThread(2);

  ASSERT_TRUE(thread->running());
  process.ClientSuspendAllThreads();
  ASSERT_FALSE(thread->running());
  process.DetachFromProcess();
  ASSERT_TRUE(thread->running());
}

TEST(DebuggedProcess, DetachFromProcessWithBreakpoints) {
  MockProcess process(nullptr, kProcessKoid, kProcessName);
  MockThread* thread = process.AddThread(1234);

  MockProcessDelegate delegate;

  Breakpoint bp1 = Breakpoint(&delegate);
  debug_ipc::BreakpointSettings settings;
  settings.type = debug_ipc::BreakpointType::kHardware;
  settings.locations.push_back(CreateLocation(process.koid(), thread->koid(), kAddress1));
  bp1.SetSettings(settings);

  ASSERT_TRUE(process.RegisterBreakpoint(&bp1, kAddress1).ok());

  Breakpoint bp2 = Breakpoint(&delegate);
  debug_ipc::BreakpointSettings settings2;
  settings2.type = debug_ipc::BreakpointType::kSoftware;
  settings2.locations.push_back(CreateLocation(process.koid(), thread->koid(), kAddress2));
  bp2.SetSettings(settings2);

  // The software breakpoint needs some memory backing it.
  process.mock_process_handle().mock_memory().AddMemory(kAddress2, GetBreakpointMemory());

  ASSERT_TRUE(process.RegisterBreakpoint(&bp2, kAddress2).ok());

  process.DetachFromProcess();

  // Now the thread should be running again and there shouldn't be any more breakpoints.
  EXPECT_TRUE(process.hardware_breakpoints().empty());
  EXPECT_TRUE(process.software_breakpoints().empty());
  EXPECT_TRUE(thread->running());
}

TEST(DebuggedProcess, WeakAttachSkipsLoaderBreakpoint) {
  MockDebugAgentHarness harness;
  DebuggedProcessCreateInfo info(std::make_unique<MockProcessHandle>(kProcessKoid, kProcessName));
  info.priority = debug_ipc::AttachConfig::Priority::kWeak;
  auto unique_process = std::make_unique<MockProcess>(harness.debug_agent(), std::move(info));
  DebuggedProcess* process = unique_process.get();

  harness.debug_agent()->InjectProcessForTest(std::move(unique_process));

  process->HandleLoaderBreakpoint(MockProcessHandle::kLoaderBreakpointAddress);

  // Should not have sent modules.
  EXPECT_TRUE(harness.stream_backend()->modules().empty());
}

TEST(DebuggedProcess, ExceptionEnsuresThreadsAreSynced) {
  MockDebugAgentHarness harness;
  auto mock_process = harness.AddProcess(kProcessKoid);

  // There should be no threads initially.
  ASSERT_TRUE(mock_process->GetThreads().empty());

  constexpr zx_koid_t kThreadKoid = 2;

  // Inject a thread to the underlying process handle object, which will be the thread that receives
  // the exception.
  mock_process->mock_process_handle().set_threads({MockThreadHandle(kThreadKoid)});

  // Dispatch the exception.
  mock_process->InjectException(std::make_unique<MockExceptionHandle>(kProcessKoid, kThreadKoid));

  // Now the DebuggedProcess should see it's child thread even though it didn't get a ThreadStarting
  // notification.
  ASSERT_EQ(mock_process->GetThreads().size(), 1u);
  EXPECT_EQ(mock_process->GetThreads()[0]->koid(), kThreadKoid);

  // We should also receive an exception notification.
  ASSERT_EQ(harness.stream_backend()->exceptions().size(), 1u);
  EXPECT_EQ(harness.stream_backend()->exceptions()[0].thread.id.process, kProcessKoid);
  EXPECT_EQ(harness.stream_backend()->exceptions()[0].thread.id.thread, kThreadKoid);
}

TEST(DebuggedProcessTest, ClientResumeUnsetsSuspendNewThreads) {
  MockDebugAgentHarness harness;

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
  mock_process->OnResume(resume_request);

  EXPECT_FALSE(debugged_thread->is_client_suspended());
  EXPECT_FALSE(debugged_thread2->is_client_suspended());

  MockThreadHandle thread3_handle(kThread3Koid);
  mock_process->mock_process_handle().AddThreadAndSendEvent(std::move(thread3_handle));
  auto* thread3 = mock_process->GetThread(kThread3Koid);
  ASSERT_TRUE(thread3);
  EXPECT_FALSE(thread3->is_client_suspended());
}

}  // namespace
}  // namespace debug_agent
