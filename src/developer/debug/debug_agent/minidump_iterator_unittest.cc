// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/minidump_iterator.h"

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/mock_debug_agent_harness.h"
#include "src/developer/debug/debug_agent/mock_process.h"
#include "src/developer/debug/shared/test_with_loop.h"

namespace debug_agent {

class MinidumpIteratorTest : public debug::TestWithLoop {
 public:
  DebugAgent* GetDebugAgent() { return harness_.debug_agent(); }

  static size_t GetCurrentIndex(const MinidumpIterator& iter) { return iter.index_; }
  static DebuggedProcess* GetCurrentProcess(const MinidumpIterator& iter) {
    return iter.current_process_;
  }

 private:
  MockDebugAgentHarness harness_;
};

TEST_F(MinidumpIteratorTest, Advance) {
  std::vector<std::unique_ptr<MockProcess>> processes;
  processes.emplace_back(std::make_unique<MockProcess>(GetDebugAgent(), 0x1));

  std::vector<DebuggedProcess*> pr;
  pr.reserve(processes.size());
  for (const auto& p : processes) {
    pr.push_back(p.get());
  }

  MinidumpIterator iter(GetDebugAgent()->GetWeakPtr(), std::move(pr));

  // Initially the index will be zero. |current_process_| will be set by the first call to
  // |Advance|.
  ASSERT_EQ(GetCurrentIndex(iter), 0u);
  ASSERT_EQ(GetCurrentProcess(iter), nullptr);

  ASSERT_TRUE(iter.Advance());

  ASSERT_EQ(GetCurrentIndex(iter), 1u);
  ASSERT_EQ(GetCurrentProcess(iter), processes[0].get());

  ASSERT_FALSE(iter.Advance());

  // Didn't increment the index, but did reset the pointer to null.
  ASSERT_EQ(GetCurrentIndex(iter), 1u);
  ASSERT_EQ(GetCurrentProcess(iter), nullptr);
}

TEST_F(MinidumpIteratorTest, NoProcesses) {
  MinidumpIterator iter(GetDebugAgent()->GetWeakPtr(), {});

  // Calling code should never call Advance with an empty vector of processes, but if it does, we
  // should behave sanely.
  ASSERT_FALSE(iter.Advance());
}

}  // namespace debug_agent
