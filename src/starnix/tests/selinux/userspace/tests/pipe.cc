// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace {

TEST(FifoTest, LabeledFromFilesystem) {
  auto scoped_current_task =
      ScopedTaskAttrResetter::SetTaskAttr("current", "test_u:test_r:pipe_test_t:s0");

  constexpr char kFifoPath[] = "/tmp/fifo_label_test";
  ASSERT_THAT(mkfifo(kFifoPath, 0600), SyscallSucceeds());

  EXPECT_THAT(GetLabel(kFifoPath), SyscallResultIsOk("test_u:object_r:test_fifo_file_t:s0"));

  fbl::unique_fd fifo(open(kFifoPath, O_RDWR));
  ASSERT_TRUE(fifo.is_valid());

  EXPECT_THAT(GetLabel(fifo.get()), SyscallResultIsOk("test_u:object_r:test_fifo_file_t:s0"));
}

// Pipes receive the creating task's context, with no transitions applied.
TEST(PipeTest, LabeledFromTask) {
  auto scoped_current_task =
      ScopedTaskAttrResetter::SetTaskAttr("current", "test_u:test_r:pipe_test_t:s0");

  int pipe_after_policy[2];
  EXPECT_THAT(pipe(pipe_after_policy), SyscallSucceeds());

  EXPECT_THAT(GetLabel(pipe_after_policy[0]), SyscallResultIsOk("test_u:test_r:pipe_test_t:s0"));
}

int g_before_policy_pipe = -1;

// Pipes created prior to policy load behave the same as those created after policy-load:
// No `type_transition` rules are applied to them, and since all tasks prior to policy load have the
// "kernel" SID, pre-policy pipes will always receive that SID as well.
TEST(PipeTest, BeforePolicyReceivesKernelContext) {
  ASSERT_THAT(ReadTaskAttr("current"), SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));

  EXPECT_THAT(GetLabel(g_before_policy_pipe),
              SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));
}

}  // namespace

extern std::string DoPrePolicyLoadWork() {
  // Create a pipe prior to policy load, to allow the test to validate the post-policy label.
  int pipe_before_policy[2];
  EXPECT_THAT(pipe(pipe_before_policy), SyscallSucceeds());
  g_before_policy_pipe = pipe_before_policy[0];

  return "pipe_policy";
}
