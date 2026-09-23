// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/resource.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "sched_policy"; }

namespace {

class ScopedTargetProcess {
 public:
  ScopedTargetProcess(std::string label) {
    test_helper::Rendezvous transitioned = test_helper::MakeRendezvous();
    test_helper::Rendezvous exit_event = test_helper::MakeRendezvous();

    exit_poker_ = std::move(exit_event.poker);

    child_ = fork_helper_.RunInForkedProcess([label, poker = std::move(transitioned.poker),
                                              holder = std::move(exit_event.holder)]() mutable {
      fit::result<int> success = WriteTaskAttr("current", label);
      poker.poke();
      holder.hold();
      _exit(success.is_error());
    });

    transitioned.holder.hold();
  }

  ~ScopedTargetProcess() { exit_poker_.poke(); }

  pid_t pid() const { return child_; }

 private:
  test_helper::Poker exit_poker_;
  test_helper::ForkHelper fork_helper_;
  pid_t child_;
};

TEST(SchedTest, SetPrioritySucceedsWithSetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_t:s0", [&] {
    EXPECT_THAT(syscall(SYS_setpriority, PRIO_PROCESS, target.pid(), 10), SyscallSucceeds());
  }));
}

TEST(SchedTest, SetPriorityDeniedWithoutSetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_no_setsched_t:s0", [&] {
    EXPECT_THAT(syscall(SYS_setpriority, PRIO_PROCESS, target.pid(), 10),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SchedTest, SchedSetschedulerSucceedsWithSetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_t:s0", [&] {
    struct sched_param param = {.sched_priority = 0};
    EXPECT_THAT(syscall(SYS_sched_setscheduler, target.pid(), SCHED_OTHER, &param),
                SyscallSucceeds());
  }));
}

TEST(SchedTest, SchedSetschedulerDeniedWithoutSetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_no_setsched_t:s0", [&] {
    struct sched_param param = {.sched_priority = 0};
    EXPECT_THAT(syscall(SYS_sched_setscheduler, target.pid(), SCHED_OTHER, &param),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SchedTest, SchedGetschedulerSucceedsWithGetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_t:s0", [&] {
    EXPECT_THAT(syscall(SYS_sched_getscheduler, target.pid()), SyscallSucceeds());
  }));
}

TEST(SchedTest, SchedGetschedulerDeniedWithoutGetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_no_getsched_t:s0", [&] {
    EXPECT_THAT(syscall(SYS_sched_getscheduler, target.pid()), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SchedTest, SchedGetaffinitySucceedsWithGetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_t:s0", [&] {
    cpu_set_t mask;
    CPU_ZERO(&mask);
    EXPECT_THAT(syscall(SYS_sched_getaffinity, target.pid(), sizeof(mask), &mask),
                SyscallSucceeds());
  }));
}

TEST(SchedTest, SchedGetaffinityDeniedWithoutGetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_no_getsched_t:s0", [&] {
    cpu_set_t mask;
    CPU_ZERO(&mask);
    EXPECT_THAT(syscall(SYS_sched_getaffinity, target.pid(), sizeof(mask), &mask),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SchedTest, SchedSetaffinitySucceedsWithSetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_t:s0", [&] {
    cpu_set_t mask;
    CPU_ZERO(&mask);
    CPU_SET(0, &mask);
    EXPECT_THAT(syscall(SYS_sched_setaffinity, target.pid(), sizeof(mask), &mask),
                SyscallSucceeds());
  }));
}

TEST(SchedTest, SchedSetaffinityDeniedWithoutSetsched) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target("test_u:test_r:test_sched_target_t:s0");

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_sched_child_no_setsched_t:s0", [&] {
    cpu_set_t mask;
    CPU_ZERO(&mask);
    CPU_SET(0, &mask);
    EXPECT_THAT(syscall(SYS_sched_setaffinity, target.pid(), sizeof(mask), &mask),
                SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace
