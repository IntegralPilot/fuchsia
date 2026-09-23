// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/process_info_iterator.h"

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/component_manager.h"
#include "src/developer/debug/debug_agent/debug_agent.h"
#include "src/developer/debug/debug_agent/debugged_process.h"
#include "src/developer/debug/debug_agent/mock_debug_agent_harness.h"
#include "src/developer/debug/debug_agent/mock_process.h"
#include "src/developer/debug/shared/test_with_loop.h"

namespace debug_agent {

namespace {
// Corresponding to processes in MockSystemInterface::CreateWithData.
constexpr zx_koid_t kProcess1JobKoid = 25;
constexpr zx_koid_t kProcess1Koid = 26;
constexpr zx_koid_t kProcess1Thread1Koid = 27;
constexpr zx_koid_t kProcess2JobKoid = 28;
constexpr zx_koid_t kProcess2Koid = 29;
constexpr zx_koid_t kProcess2Thread1Koid = 30;
constexpr zx_koid_t kProcess2Thread2Koid = 31;
}  // namespace

class ProcessInfoIteratorTest : public debug::TestWithLoop {
 public:
  // Individual tests may ignore this default set up by constructing an iterator without using
  // |all_procs_|.
  void SetUp() override {
    debug::TestWithLoop::SetUp();

    auto mock_process = harness_.AddProcess(kProcess1Koid);
    mock_process->AddThread(kProcess1Thread1Koid);
    mock_process->mock_process_handle().set_job_koid(kProcess1JobKoid);

    all_procs_.push_back(mock_process);

    mock_process = harness_.AddProcess(kProcess2Koid);
    mock_process->AddThread(kProcess2Thread1Koid);
    mock_process->AddThread(kProcess2Thread2Koid);
    mock_process->mock_process_handle().set_job_koid(kProcess2JobKoid);

    all_procs_.push_back(mock_process);
  }

  DebugAgent* GetDebugAgent() { return harness_.debug_agent(); }

  const std::vector<DebuggedProcess*>& all_procs() const { return all_procs_; }

  // Getters for iterator internals. This class doesn't own the iterator so the tests can configure
  // what is passed to the constructor.
  static size_t GetProcessIndex(const ProcessInfoIterator& iter) { return iter.process_index_; }
  static size_t GetThreadIndex(const ProcessInfoIterator& iter) { return iter.thread_index_; }
  static DebuggedProcess* GetCurrentProcess(const ProcessInfoIterator& iter) {
    return iter.current_process_;
  }
  static DebuggedThread* GetCurrentThread(const ProcessInfoIterator& iter) {
    return iter.current_thread_;
  }

 private:
  MockDebugAgentHarness harness_;

  std::vector<DebuggedProcess*> all_procs_;
};

TEST_F(ProcessInfoIteratorTest, NoProcesses) {
  // Doesn't use the processes set up in SetUp().
  ProcessInfoIterator iter(GetDebugAgent()->GetWeakPtr(), {}, std::nullopt);

  // In reality this should always produce an error to the FIDL client and never call this method,
  // but it's good to verify the edge cases anyway.
  EXPECT_FALSE(iter.Advance());
}

TEST_F(ProcessInfoIteratorTest, Advance) {
  ProcessInfoIterator iter(GetDebugAgent()->GetWeakPtr(), all_procs(), std::nullopt);

  EXPECT_EQ(GetProcessIndex(iter), 0u);
  EXPECT_EQ(GetThreadIndex(iter), 0u);

  // This is nullptr because we have not called iter.Advance() yet.
  EXPECT_EQ(GetCurrentThread(iter), nullptr);
  ASSERT_NE(GetCurrentProcess(iter), nullptr);

  auto last_proc = GetCurrentProcess(iter);
  EXPECT_EQ(GetCurrentProcess(iter)->koid(), kProcess1Koid);

  // Now advance the iterator. This will return true, the current process should remain the same and
  // current thread should be filled in now, while the thread index has advanced.
  EXPECT_TRUE(iter.Advance());
  EXPECT_EQ(GetProcessIndex(iter), 0u);
  EXPECT_EQ(GetThreadIndex(iter), 1u);
  EXPECT_EQ(GetCurrentProcess(iter), last_proc);
  ASSERT_NE(GetCurrentThread(iter), nullptr);
  EXPECT_EQ(GetCurrentThread(iter)->koid(), kProcess1Thread1Koid);

  // Advance again. This time we should have updated to the next process and the first thread in
  // that process. Note that the thread index is actually 1, not 0. This is so that the caller isn't
  // required to call Advance twice at the boundary between the end of one process's threads and
  // another process's.
  EXPECT_TRUE(iter.Advance());
  EXPECT_EQ(GetProcessIndex(iter), 1u);
  EXPECT_EQ(GetThreadIndex(iter), 1u);
  EXPECT_NE(GetCurrentProcess(iter), last_proc);
  EXPECT_EQ(GetCurrentProcess(iter)->koid(), kProcess2Koid);
  ASSERT_NE(GetCurrentThread(iter), nullptr);
  EXPECT_EQ(GetCurrentThread(iter)->koid(), kProcess2Thread1Koid);

  // Update last_proc to reflect the change.
  last_proc = GetCurrentProcess(iter);

  // Advance to the second thread.
  EXPECT_TRUE(iter.Advance());
  EXPECT_EQ(GetProcessIndex(iter), 1u);
  EXPECT_EQ(GetThreadIndex(iter), 2u);
  EXPECT_EQ(GetCurrentProcess(iter), last_proc);
  EXPECT_EQ(GetCurrentProcess(iter)->koid(), kProcess2Koid);
  ASSERT_NE(GetCurrentThread(iter), nullptr);
  EXPECT_EQ(GetCurrentThread(iter)->koid(), kProcess2Thread2Koid);

  // Advance one final time. This will return false because we've hit every thread in every process.
  EXPECT_FALSE(iter.Advance());

  // The iterator is now invalid. Callers must ensure that their work is finished.
}

TEST_F(ProcessInfoIteratorTest, Interest) {
  // No interest means nothing is captured.
  ProcessInfoIterator iter(GetDebugAgent()->GetWeakPtr(), all_procs(), std::nullopt);
  EXPECT_FALSE(iter.CaptureBacktrace());

  // An empty interest also means nothing is captured.
  fuchsia_debugger::ThreadDetailsInterest interest;
  iter = ProcessInfoIterator(GetDebugAgent()->GetWeakPtr(), all_procs(), interest);
  EXPECT_FALSE(iter.CaptureBacktrace());

  interest.backtrace(true);
  iter = ProcessInfoIterator(GetDebugAgent()->GetWeakPtr(), all_procs(), interest);

  EXPECT_TRUE(iter.CaptureBacktrace());
}

}  // namespace debug_agent
