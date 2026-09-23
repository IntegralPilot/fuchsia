// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// NOTE(nathaniel): this test is not supported to be run as not-root. We
// might choose in the future to adapt it to give some valid results when
// run as not-root, but that's not decided today.
//
// TODO: https://fxbug.dev/317285180 - drop the many GTEST_SKIPs in this
// file after another mechanism is put in place ensuring that this test
// only ever runs as root (or, if the day ever comes, support for being run
// as non-root is added).

#include <fcntl.h>
#include <grp.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mount.h>
#include <sys/resource.h>
#include <sys/select.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <linux/capability.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

constexpr uid_t kUser1Uid = 24601;
constexpr uid_t kUser2Uid = 9430;
constexpr gid_t kGid = kUser1Uid + kUser2Uid;

void Become(uid_t user_uid, uid_t effective_user_uid) {
  SAFE_SYSCALL(setgroups(0, nullptr));  // drop all supplementary groups.
  SAFE_SYSCALL(setresgid(kGid, kGid, kGid));
  SAFE_SYSCALL(setresuid(user_uid, effective_user_uid, user_uid));
}

pid_t SpawnTarget(test_helper::ForkHelper& fork_helper, test_helper::Poker ready,
                  test_helper::Holder complete, fit::function<void()> prepare) {
  return fork_helper.RunInForkedProcess([ready = std::move(ready), complete = std::move(complete),
                                         prepare = std::move(prepare)]() mutable {
    prepare();
    ready.poke();
    complete.hold();
  });
}

testing::AssertionResult CheckPriorityMinimums() {
  errno = 0;
  if (int observed = sched_get_priority_min(-1); observed != -1) {
    return testing::AssertionFailure() << "Expected -1 for an algorithm of -1 but observed "
                                       << observed << " (with errno " << errno << ")";
  }
  if (errno != EINVAL) {
    return testing::AssertionFailure()
           << "Expected EINVAL for errno for an algorithm of -1 but observed " << errno;
  }
  errno = 0;
  if (int observed = sched_get_priority_min(SCHED_OTHER); observed != 0) {
    return testing::AssertionFailure() << "Expected 0 for SCHED_OTHER but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  if (int observed = sched_get_priority_min(SCHED_FIFO); observed != 1) {
    return testing::AssertionFailure() << "Expected 1 for SCHED_FIFO but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  if (int observed = sched_get_priority_min(SCHED_RR); observed != 1) {
    return testing::AssertionFailure() << "Expected 1 for SCHED_RR but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  if (int observed = sched_get_priority_min(SCHED_BATCH); observed != 0) {
    return testing::AssertionFailure() << "Expected 0 for SCHED_BATCH but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  errno = 0;
  if (int observed = sched_get_priority_min(4); observed != -1) {
    return testing::AssertionFailure() << "Expected -1 for an algorithm of 4 but observed "
                                       << observed << " (with errno " << errno << ")";
  }
  if (errno != EINVAL) {
    return testing::AssertionFailure()
           << "Expected EINVAL for errno for an algorithm of 4 but observed " << errno;
  }
  errno = 0;
  if (int observed = sched_get_priority_min(SCHED_IDLE); observed != 0) {
    return testing::AssertionFailure() << "Expected 0 for SCHED_IDLE but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  if (int observed = sched_get_priority_min(SCHED_DEADLINE); observed != 0) {
    return testing::AssertionFailure() << "Expected 0 for SCHED_DEADLINE but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  // Skip to 8 as SCHED_EXT=7 is available in some of the contexts in which
  // we want to run this test.
  errno = 0;
  if (int observed = sched_get_priority_min(8); observed != -1) {
    return testing::AssertionFailure() << "Expected -1 for an algorithm of 8 but observed "
                                       << observed << " (with errno " << errno << ")";
  }
  if (errno != EINVAL) {
    return testing::AssertionFailure()
           << "Expected EINVAL for errno for an algorithm of 8 but observed " << errno;
  }
  return testing::AssertionSuccess();
}

TEST(SchedGetPriorityMinTest, RootCanGetMinimumPriorities) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  EXPECT_TRUE(CheckPriorityMinimums());
}

TEST(SchedGetPriorityMinTest, NonRootCanGetMinimumPriorities) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    Become(kUser1Uid, kUser1Uid);

    EXPECT_TRUE(CheckPriorityMinimums());
  });
}

testing::AssertionResult CheckPriorityMaximums() {
  errno = 0;
  if (int observed = sched_get_priority_max(-1); observed != -1) {
    return testing::AssertionFailure() << "Expected -1 for an algorithm of -1 but observed "
                                       << observed << " (with errno " << errno << ")";
  }
  if (errno != EINVAL) {
    return testing::AssertionFailure()
           << "Expected EINVAL for errno for an algorithm of -1 but observed " << errno;
  }
  errno = 0;
  if (int observed = sched_get_priority_max(SCHED_OTHER); observed != 0) {
    return testing::AssertionFailure() << "Expected 0 for SCHED_OTHER but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  if (int observed = sched_get_priority_max(SCHED_FIFO); observed != 99) {
    return testing::AssertionFailure() << "Expected 99 for SCHED_FIFO but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  if (int observed = sched_get_priority_max(SCHED_RR); observed != 99) {
    return testing::AssertionFailure() << "Expected 99 for SCHED_RR but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  if (int observed = sched_get_priority_max(SCHED_BATCH); observed != 0) {
    return testing::AssertionFailure() << "Expected 0 for SCHED_BATCH but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  errno = 0;
  if (int observed = sched_get_priority_max(4); observed != -1) {
    return testing::AssertionFailure() << "Expected -1 for an algorithm of 4 but observed "
                                       << observed << " (with errno " << errno << ")";
  }
  if (errno != EINVAL) {
    return testing::AssertionFailure()
           << "Expected EINVAL for errno for an algorithm of 4 but observed " << errno;
  }
  errno = 0;
  if (int observed = sched_get_priority_max(SCHED_IDLE); observed != 0) {
    return testing::AssertionFailure() << "Expected 0 for SCHED_IDLE but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  if (int observed = sched_get_priority_max(SCHED_DEADLINE); observed != 0) {
    return testing::AssertionFailure() << "Expected 0 for SCHED_DEADLINE but observed " << observed
                                       << " (with errno " << errno << ")";
  }
  // Skip to 8 as SCHED_EXT=7 is available in some of the contexts in which
  // we want to run this test.
  errno = 0;
  if (int observed = sched_get_priority_max(8); observed != -1) {
    return testing::AssertionFailure() << "Expected -1 for an algorithm of 8 but observed "
                                       << observed << " (with errno " << errno << ")";
  }
  if (errno != EINVAL) {
    return testing::AssertionFailure()
           << "Expected EINVAL for errno for an algorithm of 8 but observed " << errno;
  }
  return testing::AssertionSuccess();
}

TEST(SchedGetPriorityMaxTest, RootCanGetMaximumPriorities) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  EXPECT_TRUE(CheckPriorityMaximums());
}

TEST(SchedGetPriorityMaxTest, NonRootCanGetMaximumPriorities) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    Become(kUser2Uid, kUser2Uid);

    EXPECT_TRUE(CheckPriorityMaximums());
  });
}

TEST(GetPriorityTest, NonRootCanGetNicenessOfEuidUnfriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder),
                                 []() { Become(kUser2Uid, kUser2Uid); });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    // We're not testing that the observed niceness is any particular value;
    // we're just testing that the getpriority call succeeds.
    errno = 0;
    getpriority(PRIO_PROCESS, target_pid);
    EXPECT_EQ(errno, 0);

    complete.poke();
  });
}

TEST(SetPriorityTest, NonRootCanSetNicenessOfEuidFriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        // Remove rlimits from consideration in this test.
        rlimit niceness_rlimit = {
            .rlim_cur = RLIM_INFINITY,
            .rlim_max = RLIM_INFINITY,
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_NICE, &niceness_rlimit));

        // A target process with a different UID but an effective UID that
        // matches the effective UID of the effective UID of the
        // calling-the-setpriority-syscall process counts as "euid-friendly"
        // and the syscall will succeed.
        //
        // See "Linux 2.6.12 and later require the effective user ID of
        // the caller to match the real or effective user ID of the process
        // who" at setpriority(2).
        Become(kUser2Uid, kUser1Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    for (int niceness = 19; niceness >= -20; niceness--) {
      EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, niceness), 0);
      errno = 0;
      EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), niceness);
      EXPECT_EQ(errno, 0);
    }

    complete.poke();
  });
}

TEST(SetPriorityTest, NonRootCannotSetNicenessOfEuidUnfriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  constexpr int kTargetNiceness = 19;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        // Remove rlimits from consideration in this test.
        rlimit niceness_rlimit = {
            .rlim_cur = RLIM_INFINITY,
            .rlim_max = RLIM_INFINITY,
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_NICE, &niceness_rlimit));

        EXPECT_EQ(setpriority(PRIO_PROCESS, 0, kTargetNiceness), 0);

        Become(kUser2Uid, kUser2Uid);
      });

  fork_helper.RunInForkedProcess([kTargetNiceness, ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    // A niceness-setting-attempting process with the same UID as the
    // target process but a different effective UID than the target
    // process will not be able to succeed in its attempts without the
    // CAP_SYS_NICE capability (which this process does not have).
    //
    // See "Linux 2.6.12 and later require the effective user ID of
    // the caller to match the real or effective user ID of the process
    // who" at setpriority(2).
    Become(kUser2Uid, kUser1Uid);

    ready.hold();

    for (int niceness = 19; niceness >= -20; niceness--) {
      EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, niceness), -1);
      EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), kTargetNiceness);
    }

    complete.poke();
  });
}

TEST(SetPriorityTest, OutOfRangeNicenessValuesGetClamped) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    constexpr int kTooNiceNiceness = 88;           // Higher than 19.
    constexpr int kNotNiceEnoughNiceness = -1337;  // Lower than -20.

    EXPECT_EQ(setpriority(PRIO_PROCESS, 0, kTooNiceNiceness), 0);
    EXPECT_EQ(getpriority(PRIO_PROCESS, 0), 19);

    EXPECT_EQ(setpriority(PRIO_PROCESS, 0, kNotNiceEnoughNiceness), 0);
    EXPECT_EQ(getpriority(PRIO_PROCESS, 0), -20);
  });
}

TEST(SetPriorityTest, RootCanExceedRLimits) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kNicenessRlimitCur = 17;
  constexpr int kNicenessRlimitMax = kNicenessRlimitCur + 2;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit niceness_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kNicenessRlimitCur),
            .rlim_max = static_cast<rlim_t>(kNicenessRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_NICE, &niceness_rlimit));

        Become(kUser1Uid, kUser1Uid);
      });

  ready.holder.hold();

  for (int niceness = 19; niceness >= -20; niceness--) {
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, niceness), 0);
    errno = 0;
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), niceness);
    EXPECT_EQ(errno, 0);
  }

  complete.poker.poke();
}

TEST(SetPriorityTest, NonRootCannotExceedRLimits) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kNicenessRlimitCur = 24;
  constexpr int kNicenessRlimitMax = kNicenessRlimitCur + 5;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit niceness_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kNicenessRlimitCur),
            .rlim_max = static_cast<rlim_t>(kNicenessRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_NICE, &niceness_rlimit));

        Become(kUser1Uid, kUser1Uid);
      });

  ready.holder.hold();

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    for (int niceness = 19; niceness >= 20 - kNicenessRlimitCur; niceness--) {
      EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, niceness), 0);
      EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), niceness);
    }

    for (int niceness = 19 - kNicenessRlimitCur; niceness >= -20; niceness--) {
      errno = 0;
      EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, niceness), -1);
      EXPECT_EQ(errno, EACCES);

      EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), 20 - kNicenessRlimitCur);
    }

    complete.poke();
  });
}

TEST(SetPriorityTest, MaintainingAndIncreasingNicenessAllowedDespiteExceededRLimits) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kNicenessRlimitCur = 24;
  constexpr int kNicenessRlimitMax = kNicenessRlimitCur + 4;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit niceness_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kNicenessRlimitCur),
            .rlim_max = static_cast<rlim_t>(kNicenessRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_NICE, &niceness_rlimit));

        // Set this process' niceness to something *beyond* what the rlimit
        // allows.
        EXPECT_EQ(setpriority(PRIO_PROCESS, 0, -20), 0);

        Become(kUser1Uid, kUser1Uid);
      });

  ready.holder.hold();

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    for (int niceness = -20; niceness <= 19; niceness++) {
      // Increasing niceness is never disallowed by the rlimit.
      EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, niceness), 0);
      EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), niceness);

      // Maintaining niceness is never disallowed by the rlimit. It's a
      // no-op, but it's never disallowed by the rlimit.
      EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, niceness), 0);
      EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), niceness);

      // While we are in the space outside the rlimit (or just at the boundary)
      // *decreasing* the niceness is *not* allowed.
      if (niceness - 1 >= -20 && niceness - 1 < 20 - kNicenessRlimitCur) {
        errno = 0;
        EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, niceness - 1), -1);
        EXPECT_EQ(errno, EACCES);

        EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), niceness);
      }
    }

    complete.poke();
  });
}

TEST(SetPriorityTest, RLimitedAndEuidUnfriendly) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kNicenessRlimitCur = 12;
  constexpr int kNicenessRlimitMax = kNicenessRlimitCur + 3;
  constexpr int kWithinRlimitNiceness = 18;
  constexpr int kBeyondRlimitNiceness = -3;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit niceness_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kNicenessRlimitCur),
            .rlim_max = static_cast<rlim_t>(kNicenessRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_NICE, &niceness_rlimit));
        sched_param param = {.sched_priority = 0};
        SAFE_SYSCALL(sched_setscheduler(0, SCHED_OTHER, &param));
        SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kWithinRlimitNiceness));

        Become(kUser2Uid, kUser2Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    // It's the euid-unfriendliness that "wins"; attempting to set the niceness
    // fails with EPERM and the rlimit doesn't get a chance to fail with EACCES.
    errno = 0;
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, kBeyondRlimitNiceness), -1);
    EXPECT_EQ(errno, EPERM);

    complete.poke();
  });
}

TEST(SchedGetSchedulerTest, RootCanGetOwnScheduler) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  EXPECT_GE(sched_getscheduler(0), 0);
}

TEST(SchedGetSchedulerTest, RootCanGetSchedulerOfRootOwnedProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = fork_helper.RunInForkedProcess(
      [complete = std::move(complete.holder)]() mutable { complete.hold(); });

  EXPECT_GE(sched_getscheduler(target_pid), 0);

  complete.poker.poke();
}

TEST(SchedGetSchedulerTest, RootCanGetSchedulerOfNonRootOwnedProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder),
                                 []() { Become(kUser1Uid, kUser1Uid); });

  ready.holder.hold();

  EXPECT_GE(sched_getscheduler(target_pid), 0);

  complete.poker.poke();
}

TEST(SchedGetSchedulerTest, NonRootCanGetOwnScheduler) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    Become(kUser1Uid, kUser1Uid);

    EXPECT_GE(sched_getscheduler(0), 0);
  });
}

TEST(SchedGetSchedulerTest, NonRootCanGetSchedulerOfEuidFriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    Become(kUser1Uid, kUser1Uid);

    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_GE(sched_getscheduler(target_pid), 0);

    complete.poker.poke();
  });
}

TEST(SchedGetSchedulerTest, NonRootCanGetSchedulerOfEuidUnfriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder),
                                 []() { Become(kUser2Uid, kUser2Uid); });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    EXPECT_GE(sched_getscheduler(target_pid), 0);

    complete.poke();
  });
}

TEST(SchedGetSchedulerTest, ResetOnForkAppearsInReturnedValue) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    sched_param param = {.sched_priority = 0};
    EXPECT_EQ(sched_setscheduler(0, SCHED_BATCH | SCHED_RESET_ON_FORK, &param), 0);

    EXPECT_EQ(sched_getscheduler(0), SCHED_BATCH | SCHED_RESET_ON_FORK);
  });
}

testing::AssertionResult SetScheduler(pid_t pid) {
  sched_param param{};

  errno = 0;
  param.sched_priority = 0;
  if (int observed = sched_setscheduler(pid, SCHED_OTHER, &param); observed != 0) {
    return testing::AssertionFailure()
           << "Expected to be able to set scheduler of " << pid << " to SCHED_OTHER; failed with "
           << observed << " (and errno " << errno << ")";
  }
  if (int observed_policy = SAFE_SYSCALL(sched_getscheduler(pid)); observed_policy != SCHED_OTHER) {
    return testing::AssertionFailure()
           << "Expected policy of " << pid << " to be SCHED_OTHER; observed policy is "
           << observed_policy;
  }

  int min_fifo_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_FIFO));
  int max_fifo_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO));
  for (auto priority = min_fifo_priority; priority <= max_fifo_priority; priority++) {
    param.sched_priority = priority;
    if (int observed = sched_setscheduler(pid, SCHED_FIFO, &param); observed != 0) {
      return testing::AssertionFailure()
             << "Expected to be able to set scheduler of " << pid << " to SCHED_FIFO with priority "
             << priority << "; failed with " << observed << " (and errno " << errno << ")";
    }
    if (int observed_policy = SAFE_SYSCALL(sched_getscheduler(pid));
        observed_policy != SCHED_FIFO) {
      return testing::AssertionFailure()
             << "Expected policy of " << pid << " to be SCHED_FIFO; observed policy is "
             << observed_policy;
    }
    sched_param observed_param = {.sched_priority = -1};
    SAFE_SYSCALL(sched_getparam(pid, &observed_param));
    if (observed_param.sched_priority != priority) {
      return testing::AssertionFailure()
             << "Expected priority of " << pid << " to be " << priority << "; observed priority is "
             << observed_param.sched_priority;
    }
  }

  int min_rr_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_RR));
  int max_rr_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_RR));
  for (auto priority = min_rr_priority; priority <= max_rr_priority; priority++) {
    param.sched_priority = priority;
    if (int observed = sched_setscheduler(pid, SCHED_RR, &param); observed != 0) {
      return testing::AssertionFailure()
             << "Expected to be able to set scheduler of " << pid << " to SCHED_RR with priority "
             << priority << "; failed with " << observed << " (and errno " << errno << ")";
    }
    if (int observed_policy = SAFE_SYSCALL(sched_getscheduler(pid)); observed_policy != SCHED_RR) {
      return testing::AssertionFailure()
             << "Expected policy of " << pid << " to be SCHED_RR; observed policy is "
             << observed_policy;
    }
    sched_param observed_param = {.sched_priority = -1};
    SAFE_SYSCALL(sched_getparam(pid, &observed_param));
    if (observed_param.sched_priority != priority) {
      return testing::AssertionFailure()
             << "Expected priority of " << pid << " to be " << priority << "; observed priority is "
             << observed_param.sched_priority;
    }
  }

  errno = 0;
  param.sched_priority = 0;
  if (int observed = sched_setscheduler(pid, SCHED_BATCH, &param); observed != 0) {
    return testing::AssertionFailure()
           << "Expected to be able to set scheduler of " << pid << " to SCHED_BATCH; failed with "
           << observed << " (and errno " << errno << ")";
  }
  if (int observed_policy = SAFE_SYSCALL(sched_getscheduler(pid)); observed_policy != SCHED_BATCH) {
    return testing::AssertionFailure()
           << "Expected policy of " << pid << " to be SCHED_BATCH; observed policy is "
           << observed_policy;
  }

  errno = 0;
  param.sched_priority = 0;
  if (int observed = sched_setscheduler(pid, SCHED_IDLE, &param); observed != 0) {
    return testing::AssertionFailure()
           << "Expected to be able to set scheduler of " << pid << " to SCHED_IDLE; failed with "
           << observed << " (and errno " << errno << ")";
  }
  if (int observed_policy = SAFE_SYSCALL(sched_getscheduler(pid)); observed_policy != SCHED_IDLE) {
    return testing::AssertionFailure()
           << "Expected policy of " << pid << " to be SCHED_IDLE; observed policy is "
           << observed_policy;
  }

  return testing::AssertionSuccess();
}

TEST(SchedSetSchedulerTest, InvalidArguments) {
  sched_param param{};

  // See "Invalid arguments: pid is negative or param is NULL" at
  // sched_setscheduler(2).
  errno = 0;
  EXPECT_EQ(sched_setscheduler(-1, SCHED_RR, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  EXPECT_EQ(sched_setscheduler(-7, SCHED_RR, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  EXPECT_EQ(sched_setscheduler(0, SCHED_RR, nullptr), -1);
  EXPECT_EQ(errno, EINVAL);

  // See "policy is not one of the recognized policies" at sched_setscheduler(7).
  errno = 0;
  EXPECT_EQ(sched_setscheduler(0, 4, nullptr), -1);
  EXPECT_EQ(errno, EINVAL);

  // Real-time priority differs from niceness in that attempting to set a value
  // outside the system-supported range will be rejected rather than clamped.
  //
  // (This is also "param does not make sense for the specified policy" at
  // sched_setscheduler(7).)
  errno = 0;
  param.sched_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_RR)) - 20;
  EXPECT_EQ(sched_setscheduler(0, SCHED_RR, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  param.sched_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_RR)) - 1;
  EXPECT_EQ(sched_setscheduler(0, SCHED_RR, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  param.sched_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_RR)) + 1;
  EXPECT_EQ(sched_setscheduler(0, SCHED_RR, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  param.sched_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_RR)) + 20;
  EXPECT_EQ(sched_setscheduler(0, SCHED_RR, &param), -1);
  EXPECT_EQ(errno, EINVAL);

  // For non-real-time policies, the priority must be zero. (See "For each of the
  // above policies, param->sched_priority must be 0" at sched_setscheduler(7).)
  errno = 0;
  param.sched_priority = -1;
  EXPECT_EQ(sched_setscheduler(0, SCHED_OTHER, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  param.sched_priority = 1;
  EXPECT_EQ(sched_setscheduler(0, SCHED_OTHER, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  param.sched_priority = -1;
  EXPECT_EQ(sched_setscheduler(0, SCHED_BATCH, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  param.sched_priority = 1;
  EXPECT_EQ(sched_setscheduler(0, SCHED_BATCH, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  param.sched_priority = -1;
  EXPECT_EQ(sched_setscheduler(0, SCHED_IDLE, &param), -1);
  EXPECT_EQ(errno, EINVAL);
  errno = 0;
  param.sched_priority = 1;
  EXPECT_EQ(sched_setscheduler(0, SCHED_IDLE, &param), -1);
  EXPECT_EQ(errno, EINVAL);

  // SCHED_DEADLINE is not usable with sched_setscheduler; see "To set and fetch
  // this policy and associated attributes, one must use the Linux-specific
  // sched_setattr(2) and sched_getattr(2) system calls" at sched(7).
  errno = 0;
  param.sched_priority = 0;
  EXPECT_EQ(sched_setscheduler(0, SCHED_DEADLINE, &param), -1);
  EXPECT_EQ(errno, EINVAL);
}

TEST(SchedSetSchedulerTest, RootCanSetOwnScheduler) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() { EXPECT_TRUE(SetScheduler(0)); });
}

TEST(SchedSetSchedulerTest, RootCanSetSchedulerOfRootOwnedProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_TRUE(SetScheduler(target_pid));

    complete.poker.poke();
  });
}

TEST(SchedSetSchedulerTest, RootCanSetSchedulerOfNonRootOwnedProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous ready = test_helper::MakeRendezvous();
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid =
        SpawnTarget(target_process_fork_helper, std::move(ready.poker), std::move(complete.holder),
                    []() { Become(kUser1Uid, kUser1Uid); });

    ready.holder.hold();

    EXPECT_TRUE(SetScheduler(target_pid));

    complete.poker.poke();
  });
}

TEST(SchedSetSchedulerTest, NonRootCanSetOwnScheduler) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    // Remove rlimits from consideration in this test.
    rlimit rtpriority_rlimit = {
        .rlim_cur = RLIM_INFINITY,
        .rlim_max = RLIM_INFINITY,
    };
    SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

    Become(kUser1Uid, kUser1Uid);

    EXPECT_TRUE(SetScheduler(0));
  });
}

TEST(SchedSetSchedulerTest, NonRootCanSetSchedulerOfEuidFriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        // Remove rlimits from consideration in this test.
        rlimit rtpriority_rlimit = {
            .rlim_cur = RLIM_INFINITY,
            .rlim_max = RLIM_INFINITY,
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

        Become(kUser1Uid, kUser1Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    EXPECT_TRUE(SetScheduler(target_pid));

    complete.poke();
  });
}

TEST(SchedSetSchedulerTest, NonRootCannotSetSchedulerOfEuidUnfriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        // Remove rlimits from consideration in this test.
        rlimit rtpriority_rlimit = {
            .rlim_cur = RLIM_INFINITY,
            .rlim_max = RLIM_INFINITY,
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

        Become(kUser2Uid, kUser2Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    sched_param param{};

    errno = 0;
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_OTHER, &param), -1);
    EXPECT_EQ(errno, EPERM);

    int min_fifo_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_FIFO));
    int max_fifo_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO));
    for (auto priority = min_fifo_priority; priority <= max_fifo_priority; priority++) {
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), -1);
      EXPECT_EQ(errno, EPERM);
    }

    int min_rr_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_RR));
    int max_rr_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_RR));
    for (auto priority = min_rr_priority; priority <= max_rr_priority; priority++) {
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_RR, &param), -1);
      EXPECT_EQ(errno, EPERM);
    }

    errno = 0;
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_BATCH, &param), -1);
    EXPECT_EQ(errno, EPERM);

    errno = 0;
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_IDLE, &param), -1);
    EXPECT_EQ(errno, EPERM);

    complete.poke();
  });
}

TEST(SchedSetSchedulerTest, RootCanExceedRLimits) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kRtpriorityRlimitCur = 47;
  constexpr int kRtpriorityRlimitMax = kRtpriorityRlimitCur + 17;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit rtpriority_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kRtpriorityRlimitCur),
            .rlim_max = static_cast<rlim_t>(kRtpriorityRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

        Become(kUser1Uid, kUser1Uid);
      });

  ready.holder.hold();

  EXPECT_TRUE(SetScheduler(target_pid));

  complete.poker.poke();
}

TEST(SchedSetSchedulerTest, NonRootCannotExceedRLimits) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kRtpriorityRlimitCur = 43;
  constexpr int kRtpriorityRlimitMax = kRtpriorityRlimitCur + 11;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit rtpriority_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kRtpriorityRlimitCur),
            .rlim_max = static_cast<rlim_t>(kRtpriorityRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

        Become(kUser1Uid, kUser1Uid);
      });

  ready.holder.hold();

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    sched_param param{};

    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_OTHER, &param), 0);
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(target_pid)), SCHED_OTHER);

    int min_fifo_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_FIFO));
    int max_fifo_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO));
    for (auto priority = min_fifo_priority; priority <= kRtpriorityRlimitCur; priority++) {
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), 0);
      EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(target_pid)), SCHED_FIFO);
      sched_param observed_param = {.sched_priority = -1};
      SAFE_SYSCALL(sched_getparam(target_pid, &observed_param));
      EXPECT_EQ(priority, observed_param.sched_priority);
    }
    for (auto priority = kRtpriorityRlimitCur + 1; priority <= max_fifo_priority; priority++) {
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), -1);
      EXPECT_EQ(errno, EPERM);
    }

    int min_rr_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_RR));
    int max_rr_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_RR));
    for (auto priority = min_rr_priority; priority <= kRtpriorityRlimitCur; priority++) {
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_RR, &param), 0);
      EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(target_pid)), SCHED_RR);
      sched_param observed_param = {.sched_priority = -1};
      SAFE_SYSCALL(sched_getparam(target_pid, &observed_param));
      EXPECT_EQ(priority, observed_param.sched_priority);
    }
    for (auto priority = kRtpriorityRlimitCur + 1; priority <= max_rr_priority; priority++) {
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_RR, &param), -1);
      EXPECT_EQ(errno, EPERM);
    }

    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_BATCH, &param), 0);
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(target_pid)), SCHED_BATCH);

    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_IDLE, &param), 0);
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(target_pid)), SCHED_IDLE);

    complete.poke();
  });
}

TEST(SchedSetSchedulerTest, MaintainingAndDecreasingPriorityAllowedDespiteExceededRLimits) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Divide the valid-priority-values range into three subranges each
  // large-enough for the priority to be stepped down without crossing
  // into the next range (so: each is at least two). These values are
  // otherwise arbitrary. (And the max value ought not matter at all
  // unless the *nix-under-test has a defect that makes it matter.)
  constexpr int kRtpriorityRlimitCur = 37;
  constexpr int kRtpriorityRlimitMax = kRtpriorityRlimitCur + 19;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit rtpriority_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kRtpriorityRlimitCur),
            .rlim_max = static_cast<rlim_t>(kRtpriorityRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

        // Set this process' priority to something *beyond* what the rlimit
        // allows.
        sched_param param = {.sched_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO))};
        EXPECT_EQ(sched_setscheduler(0, SCHED_FIFO, &param), 0);

        Become(kUser1Uid, kUser1Uid);
      });

  ready.holder.hold();

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    int min_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_FIFO));
    int max_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO));

    for (auto priority = max_priority; priority >= min_priority; priority--) {
      sched_param param{};
      sched_param observed_param{};

      // Lowering the priority is never disallowed by the rlimit.
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), 0);
      EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(target_pid)), SCHED_FIFO);
      observed_param.sched_priority = -1;
      SAFE_SYSCALL(sched_getparam(target_pid, &observed_param));
      EXPECT_EQ(priority, observed_param.sched_priority);

      // Maintaining the priority is never disallowed by the rlimit. It's a
      // no-op, but it's never disallowed by the rlimit.
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), 0);
      EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(target_pid)), SCHED_FIFO);
      observed_param.sched_priority = -1;
      SAFE_SYSCALL(sched_getparam(target_pid, &observed_param));
      EXPECT_EQ(priority, observed_param.sched_priority);

      // Raising the priority to a value beyond the rlimit is disallowed.
      if (priority + 1 <= max_priority && priority + 1 > kRtpriorityRlimitCur) {
        errno = 0;
        param.sched_priority = priority + 1;
        EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), -1);
        EXPECT_EQ(errno, EPERM);

        EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(target_pid)), SCHED_FIFO);
        observed_param.sched_priority = -1;
        SAFE_SYSCALL(sched_getparam(target_pid, &observed_param));
        EXPECT_EQ(priority, observed_param.sched_priority);
      }
    }

    complete.poke();
  });
}

TEST(SchedSetSchedulerTest, RLimitedAndEuidUnfriendly) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kRtpriorityRlimitCur = 47;
  constexpr int kRtpriorityRlimitMax = kRtpriorityRlimitCur + 4;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit rtpriority_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kRtpriorityRlimitCur),
            .rlim_max = static_cast<rlim_t>(kRtpriorityRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

        Become(kUser2Uid, kUser2Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    sched_param param{};

    errno = 0;
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_OTHER, &param), -1);
    EXPECT_EQ(errno, EPERM);

    int min_fifo_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_FIFO));
    int max_fifo_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO));
    for (auto priority = min_fifo_priority; priority <= max_fifo_priority; priority++) {
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), -1);
      EXPECT_EQ(errno, EPERM);
    }

    int min_rr_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_RR));
    int max_rr_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_RR));
    for (auto priority = min_rr_priority; priority <= max_rr_priority; priority++) {
      errno = 0;
      param.sched_priority = priority;
      EXPECT_EQ(sched_setscheduler(target_pid, SCHED_RR, &param), -1);
      EXPECT_EQ(errno, EPERM);
    }

    errno = 0;
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_BATCH, &param), -1);
    EXPECT_EQ(errno, EPERM);

    errno = 0;
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_IDLE, &param), -1);
    EXPECT_EQ(errno, EPERM);

    complete.poke();
  });
}

TEST(SchedSetSchedulerTest, RootCanClearResetOnFork) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    sched_param param = {.sched_priority = 2};

    EXPECT_EQ(sched_setscheduler(0, SCHED_FIFO | SCHED_RESET_ON_FORK, &param), 0);
    EXPECT_EQ(sched_getscheduler(0), SCHED_FIFO | SCHED_RESET_ON_FORK);

    EXPECT_EQ(sched_setscheduler(0, SCHED_FIFO, &param), 0);
    EXPECT_EQ(sched_getscheduler(0), SCHED_FIFO);
  });
}

TEST(SchedSetSchedulerTest, NonRootCannotClearResetOnFork) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    sched_param param = {.sched_priority = 2};
    EXPECT_EQ(sched_setscheduler(0, SCHED_FIFO | SCHED_RESET_ON_FORK, &param), 0);

    Become(kUser1Uid, kUser1Uid);

    errno = 0;
    EXPECT_EQ(sched_setscheduler(0, SCHED_FIFO, &param), -1);
    EXPECT_EQ(errno, EPERM);
  });
}

TEST(SchedSetSchedulerTest, ResetOnForkShiftsToNonRealTimePolicy) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    sched_param param = {.sched_priority = 2};
    EXPECT_EQ(sched_setscheduler(0, SCHED_FIFO | SCHED_RESET_ON_FORK, &param), 0);

    Become(kUser1Uid, kUser1Uid);

    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_EQ(sched_getscheduler(target_pid), SCHED_OTHER);

    complete.poker.poke();
  });
}

TEST(SchedSetSchedulerTest, ResetOnForkShiftsToNonRealTimePolicyEvenForRoot) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    sched_param param = {.sched_priority = 2};
    EXPECT_EQ(sched_setscheduler(0, SCHED_FIFO | SCHED_RESET_ON_FORK, &param), 0);

    // No transition to an ordinary user here; we remain root!

    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_EQ(sched_getscheduler(target_pid), SCHED_OTHER);

    complete.poker.poke();
  });
}

TEST(SchedSetSchedulerTest, ResetOnForkZeroesNegativeNiceness) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    sched_param param = {.sched_priority = 0};
    EXPECT_EQ(sched_setscheduler(0, SCHED_BATCH | SCHED_RESET_ON_FORK, &param), 0);
    EXPECT_EQ(setpriority(PRIO_PROCESS, 0, -10), 0);

    Become(kUser1Uid, kUser1Uid);

    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_EQ(sched_getscheduler(target_pid), SCHED_BATCH);
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), 0);

    complete.poker.poke();
  });
}

TEST(SchedSetSchedulerTest, ResetOnForkPreservesPositiveNiceness) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    constexpr int kPolicyVariant = SCHED_IDLE;
    constexpr int kPositiveNiceness = 10;

    sched_param param = {.sched_priority = 0};
    EXPECT_EQ(sched_setscheduler(0, kPolicyVariant | SCHED_RESET_ON_FORK, &param), 0);
    EXPECT_EQ(setpriority(PRIO_PROCESS, 0, kPositiveNiceness), 0);

    Become(kUser1Uid, kUser1Uid);

    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();
    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_EQ(sched_getscheduler(target_pid), kPolicyVariant);
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), kPositiveNiceness);

    complete.poker.poke();
  });
}

TEST(SchedGetParamTest, RootCanGetOwnParam) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  sched_param observed_param{};
  observed_param.sched_priority = -1;

  EXPECT_EQ(sched_getparam(0, &observed_param), 0);
  EXPECT_GE(observed_param.sched_priority, 0);
}

TEST(SchedGetParamTest, RootCanGetParamOfRootOwnedProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = fork_helper.RunInForkedProcess(
      [complete = std::move(complete.holder)]() mutable { complete.hold(); });

  sched_param observed_param{};
  observed_param.sched_priority = -1;

  EXPECT_EQ(sched_getparam(target_pid, &observed_param), 0);
  EXPECT_GE(observed_param.sched_priority, 0);

  complete.poker.poke();
}

TEST(SchedGetParamTest, RootCanGetParamOfNonRootOwnedProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder),
                                 []() { Become(kUser1Uid, kUser1Uid); });

  ready.holder.hold();

  sched_param observed_param{};
  observed_param.sched_priority = -1;

  EXPECT_EQ(sched_getparam(target_pid, &observed_param), 0);
  EXPECT_GE(observed_param.sched_priority, 0);

  complete.poker.poke();
}

TEST(SchedGetParamTest, NonRootCanGetOwnScheduler) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    Become(kUser1Uid, kUser1Uid);

    sched_param observed_param{};
    observed_param.sched_priority = -1;

    EXPECT_EQ(sched_getparam(0, &observed_param), 0);
    EXPECT_GE(observed_param.sched_priority, 0);
  });
}

TEST(SchedGetParamTest, NonRootCanGetParamOfEuidFriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    Become(kUser1Uid, kUser1Uid);

    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    sched_param observed_param{};
    observed_param.sched_priority = -1;

    EXPECT_EQ(sched_getparam(target_pid, &observed_param), 0);
    EXPECT_GE(observed_param.sched_priority, 0);

    complete.poker.poke();
  });
}

TEST(SchedGetParamTest, NonRootCanGetParamOfEuidUnfriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder),
                                 []() { Become(kUser2Uid, kUser2Uid); });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    sched_param observed_param{};
    observed_param.sched_priority = -1;

    EXPECT_EQ(sched_getparam(target_pid, &observed_param), 0);
    EXPECT_GE(observed_param.sched_priority, 0);

    complete.poke();
  });
}

testing::AssertionResult SetParam(pid_t pid) {
  sched_param param{};
  sched_param observed_param{};

  param.sched_priority = 0;
  SAFE_SYSCALL(sched_setscheduler(pid, SCHED_OTHER, &param));
  param.sched_priority = 0;
  if (int observed = sched_setparam(pid, &param); observed != 0) {
    return testing::AssertionFailure()
           << "Expected to be able to perform the nullipotent operation of setting to 0 the priority of "
           << pid
           << " which already has priority 0 (and is of a policy that doesn't use priority anyway!)";
  }
  observed_param.sched_priority = -1;
  SAFE_SYSCALL(sched_getparam(pid, &observed_param));
  if (observed_param.sched_priority != 0) {
    return testing::AssertionFailure()
           << "Expected that after successfully setting priority of " << pid
           << " to 0, retrieving the priority would also show it to be 0, but observed "
           << observed_param.sched_priority;
  }

  int min_fifo_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_FIFO));
  int max_fifo_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO));
  param.sched_priority = min_fifo_priority;
  SAFE_SYSCALL(sched_setscheduler(pid, SCHED_FIFO, &param));
  for (auto priority = min_fifo_priority; priority <= max_fifo_priority; priority++) {
    param.sched_priority = priority;
    if (int observed = sched_setparam(pid, &param); observed != 0) {
      return testing::AssertionFailure() << "Expected to be able to set the priority of " << pid
                                         << " to " << priority << " but failed with " << observed;
    }
    observed_param.sched_priority = -1;
    SAFE_SYSCALL(sched_getparam(pid, &observed_param));
    if (observed_param.sched_priority != priority) {
      return testing::AssertionFailure()
             << "Expected that after successfully setting priority of " << pid << " to " << priority
             << ", retrieving the priority would also show it to be " << priority
             << ", but observed " << observed_param.sched_priority;
    }
  }

  int min_rr_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_RR));
  int max_rr_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_RR));
  param.sched_priority = min_rr_priority;
  SAFE_SYSCALL(sched_setscheduler(pid, SCHED_RR, &param));
  for (auto priority = min_rr_priority; priority <= max_rr_priority; priority++) {
    param.sched_priority = priority;
    if (int observed = sched_setparam(pid, &param); observed != 0) {
      return testing::AssertionFailure() << "Expected to be able to set the priority of " << pid
                                         << " to " << priority << " but failed with " << observed;
    }
    observed_param.sched_priority = -1;
    SAFE_SYSCALL(sched_getparam(pid, &observed_param));
    if (observed_param.sched_priority != priority) {
      return testing::AssertionFailure()
             << "Expected that after successfully setting priority of " << pid << " to " << priority
             << ", retrieving the priority would also show it to be " << priority
             << ", but observed " << observed_param.sched_priority;
    }
  }

  param.sched_priority = 0;
  SAFE_SYSCALL(sched_setscheduler(pid, SCHED_BATCH, &param));
  param.sched_priority = 0;
  if (int observed = sched_setparam(pid, &param); observed != 0) {
    return testing::AssertionFailure()
           << "Expected to be able to perform the nullipotent operation of setting to 0 the priority of "
           << pid
           << " which already has priority 0 (and is of a policy that doesn't use priority anyway!)";
  }
  observed_param.sched_priority = -1;
  SAFE_SYSCALL(sched_getparam(pid, &observed_param));
  if (observed_param.sched_priority != 0) {
    return testing::AssertionFailure()
           << "Expected that after successfully setting priority of " << pid
           << " to 0, retrieving the priority would also show it to be 0, but observed "
           << observed_param.sched_priority;
  }

  param.sched_priority = 0;
  SAFE_SYSCALL(sched_setscheduler(pid, SCHED_IDLE, &param));
  param.sched_priority = 0;
  if (int observed = sched_setparam(pid, &param); observed != 0) {
    return testing::AssertionFailure()
           << "Expected to be able to perform the nullipotent operation of setting to 0 the priority of "
           << pid
           << " which already has priority 0 (and is of a policy that doesn't use priority anyway!)";
  }
  observed_param.sched_priority = -1;
  SAFE_SYSCALL(sched_getparam(pid, &observed_param));
  if (observed_param.sched_priority != 0) {
    return testing::AssertionFailure()
           << "Expected that after successfully setting priority of " << pid
           << " to 0, retrieving the priority would also show it to be 0, but observed "
           << observed_param.sched_priority;
  }

  return testing::AssertionSuccess();
}

TEST(SchedSetParamTest, InvalidArgument) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    sched_param param{};

    // See "Invalid arguments: param is NULL or pid is negative" at
    // sched_setparam(2).
    errno = 0;
    EXPECT_EQ(sched_setparam(-1, &param), -1);
    EXPECT_EQ(errno, EINVAL);
    errno = 0;
    EXPECT_EQ(sched_setparam(-7, &param), -1);
    EXPECT_EQ(errno, EINVAL);
    errno = 0;
    EXPECT_EQ(sched_setparam(0, nullptr), -1);
    EXPECT_EQ(errno, EINVAL);

    // See "The argument param does not make sense for the current
    // scheduling policy" at sched_setparam(2).
    int min_rr_priority = SAFE_SYSCALL(sched_get_priority_min(SCHED_RR));
    param.sched_priority = min_rr_priority;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_RR, &param));
    errno = 0;
    param.sched_priority = min_rr_priority - 20;
    EXPECT_EQ(sched_setparam(0, &param), -1);
    EXPECT_EQ(errno, EINVAL);
  });
}

TEST(SchedSetParamTest, InvalidArgumentAndEuidUnfriendly) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        // Remove rlimits from consideration in this test.
        rlimit rtpriority_rlimit = {
            .rlim_cur = RLIM_INFINITY,
            .rlim_max = RLIM_INFINITY,
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

        sched_param param = {.sched_priority = 25};
        SAFE_SYSCALL(sched_setscheduler(0, SCHED_RR, &param));

        Become(kUser2Uid, kUser2Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    // 0 is a valid priority for some scheduling policies, but not for
    // SCHED_RR, so the EINVAL here (rather than EPERM) tells us that the
    // "this priority doesn't make sense for the current policy" "wins"
    // over "the caller does not have permission sufficient to change the
    // target".
    errno = 0;
    sched_param param{.sched_priority = 0};
    EXPECT_EQ(sched_setparam(target_pid, &param), -1);
    EXPECT_EQ(errno, EINVAL);

    complete.poke();
  });
}

TEST(SchedSetParamTest, RootCanSetOwnParam) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() { EXPECT_TRUE(SetParam(0)); });
}

TEST(SchedSetParamTest, RootCanSetParamOfRootOwnedProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_TRUE(SetParam(target_pid));

    complete.poker.poke();
  });
}

TEST(SchedSetParamTest, RootCanSetParamOfNonRootOwnedProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous ready = test_helper::MakeRendezvous();
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();

    pid_t target_pid =
        SpawnTarget(target_process_fork_helper, std::move(ready.poker), std::move(complete.holder),
                    []() { Become(kUser1Uid, kUser1Uid); });

    ready.holder.hold();

    EXPECT_TRUE(SetParam(target_pid));

    complete.poker.poke();
  });
}

TEST(SchedSetParamTest, NonRootCanSetOwnParam) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    // Remove rlimits from consideration in this test.
    rlimit rtpriority_rlimit = {
        .rlim_cur = RLIM_INFINITY,
        .rlim_max = RLIM_INFINITY,
    };
    SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

    Become(kUser1Uid, kUser1Uid);

    EXPECT_TRUE(SetParam(0));
  });
}

TEST(SchedSetParamTest, NonRootCanSetParamOfEuidFriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        // Remove rlimits from consideration in this test.
        rlimit rtpriority_rlimit = {
            .rlim_cur = RLIM_INFINITY,
            .rlim_max = RLIM_INFINITY,
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

        Become(kUser1Uid, kUser1Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    EXPECT_TRUE(SetParam(target_pid));

    complete.poke();
  });
}

TEST(SchedSetParamTest, NonRootCannotSetParamOfEuidUnfriendlyProcess) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  constexpr int kPolicyVariant = SCHED_FIFO;

  int min_priority = SAFE_SYSCALL(sched_get_priority_min(kPolicyVariant));
  int max_priority = SAFE_SYSCALL(sched_get_priority_max(kPolicyVariant));

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder),
                                 [min_priority]() {
                                   // Remove rlimits from consideration in this test.
                                   rlimit rtpriority_rlimit = {
                                       .rlim_cur = RLIM_INFINITY,
                                       .rlim_max = RLIM_INFINITY,
                                   };
                                   SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

                                   sched_param param = {.sched_priority = min_priority};
                                   SAFE_SYSCALL(sched_setscheduler(0, kPolicyVariant, &param));

                                   Become(kUser2Uid, kUser2Uid);
                                 });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid, min_priority,
                                  max_priority]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    for (int priority = min_priority; priority <= max_priority; priority++) {
      errno = 0;
      sched_param param = {.sched_priority = priority};
      EXPECT_EQ(sched_setparam(target_pid, &param), -1);
      EXPECT_EQ(errno, EPERM);
      sched_param observed_param = {.sched_priority = -1};
      SAFE_SYSCALL(sched_getparam(target_pid, &observed_param));
      EXPECT_EQ(observed_param.sched_priority, min_priority);
    }

    complete.poke();
  });
}

TEST(SchedSetParamTest, RootCanExceedRLimits) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  constexpr int kPolicyVariant = SCHED_FIFO;

  // Not the min, not the max, otherwise arbitrary.
  int min_priority = SAFE_SYSCALL(sched_get_priority_min(kPolicyVariant));
  int max_priority = SAFE_SYSCALL(sched_get_priority_max(kPolicyVariant));
  int rtpriority_rlimit_cur = ((min_priority + max_priority) / 2) + 1;
  int rtpriority_rlimit_max = rtpriority_rlimit_cur + 8;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder),
                                 [rtpriority_rlimit_cur, rtpriority_rlimit_max]() {
                                   rlimit rtpriority_rlimit = {
                                       .rlim_cur = static_cast<rlim_t>(rtpriority_rlimit_cur),
                                       .rlim_max = static_cast<rlim_t>(rtpriority_rlimit_max),
                                   };
                                   SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

                                   Become(kUser1Uid, kUser1Uid);
                                 });

  ready.holder.hold();

  EXPECT_TRUE(SetParam(target_pid));

  complete.poker.poke();
}

TEST(SchedSetParamTest, NonRootCannotExceedRLimits) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  constexpr int kPolicyVariant = SCHED_FIFO;
  int min_priority = SAFE_SYSCALL(sched_get_priority_min(kPolicyVariant));
  int max_priority = SAFE_SYSCALL(sched_get_priority_max(kPolicyVariant));
  int rtpriority_rlimit_cur = (min_priority + max_priority) / 2;
  int rtpriority_rlimit_max = rtpriority_rlimit_cur + 7;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid = SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder),
                                 [min_priority, rtpriority_rlimit_cur, rtpriority_rlimit_max]() {
                                   rlimit rtpriority_rlimit = {
                                       .rlim_cur = static_cast<rlim_t>(rtpriority_rlimit_cur),
                                       .rlim_max = static_cast<rlim_t>(rtpriority_rlimit_max),
                                   };
                                   SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));

                                   sched_param param = {.sched_priority = min_priority};
                                   SAFE_SYSCALL(sched_setscheduler(0, kPolicyVariant, &param));

                                   Become(kUser1Uid, kUser1Uid);
                                 });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), min_priority, max_priority,
                                  rtpriority_rlimit_cur, target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    for (auto priority = min_priority; priority <= rtpriority_rlimit_cur; priority++) {
      errno = 0;
      sched_param param = {.sched_priority = priority};
      EXPECT_EQ(sched_setparam(target_pid, &param), 0);
      sched_param observed_param = {.sched_priority = -1};
      SAFE_SYSCALL(sched_getparam(target_pid, &observed_param));
      EXPECT_EQ(priority, observed_param.sched_priority);
    }
    for (auto priority = rtpriority_rlimit_cur + 1; priority <= max_priority; priority++) {
      errno = 0;
      sched_param param = {.sched_priority = priority};
      EXPECT_EQ(sched_setparam(target_pid, &param), -1);
      EXPECT_EQ(errno, EPERM);
      sched_param observed_param = {.sched_priority = -1};
      SAFE_SYSCALL(sched_getparam(target_pid, &observed_param));
      EXPECT_EQ(observed_param.sched_priority, rtpriority_rlimit_cur);
    }

    complete.poke();
  });
}

TEST(SuccessivePoliciesTest, NicenessIsPreservedByPoliciesThatDoNotUseNiceness) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    constexpr int kNiceness = 10;
    sched_param param{};
    sched_param observed_param{};

    param.sched_priority = 0;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_BATCH, &param));
    SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kNiceness));
    observed_param.sched_priority = -1;
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_BATCH);
    SAFE_SYSCALL(sched_getparam(0, &observed_param));
    EXPECT_EQ(observed_param.sched_priority, 0);
    errno = 0;
    EXPECT_EQ(SAFE_SYSCALL(getpriority(PRIO_PROCESS, 0)), kNiceness);
    EXPECT_EQ(errno, 0);

    param.sched_priority = 33;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_FIFO, &param));
    observed_param.sched_priority = -1;
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_FIFO);
    SAFE_SYSCALL(sched_getparam(0, &observed_param));
    EXPECT_EQ(observed_param.sched_priority, 33);

    param.sched_priority = 0;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_OTHER, &param));
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_OTHER);
    EXPECT_EQ(SAFE_SYSCALL(getpriority(PRIO_PROCESS, 0)), kNiceness);
  });
}

TEST(SuccessivePoliciesTest, UnusedNicenessCanBeAccessed) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    constexpr int kNiceness = 16;
    sched_param param{};
    sched_param observed_param{};

    param.sched_priority = 0;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_BATCH, &param));
    SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kNiceness));
    observed_param.sched_priority = -1;
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_BATCH);
    SAFE_SYSCALL(sched_getparam(0, &observed_param));
    EXPECT_EQ(observed_param.sched_priority, 0);
    errno = 0;
    EXPECT_EQ(SAFE_SYSCALL(getpriority(PRIO_PROCESS, 0)), kNiceness);
    EXPECT_EQ(errno, 0);

    param.sched_priority = 33;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_FIFO, &param));
    observed_param.sched_priority = -1;
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_FIFO);
    SAFE_SYSCALL(sched_getparam(0, &observed_param));
    EXPECT_EQ(observed_param.sched_priority, 33);

    EXPECT_EQ(getpriority(PRIO_PROCESS, 0), kNiceness);
  });
}

TEST(SuccessivePoliciesTest, UnusedNicenessCanBeChanged) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  constexpr int kFirstNiceness = 12;
  constexpr int kSecondNiceness = kFirstNiceness + 2;

  sched_param param{};
  sched_param observed_param{};

  param.sched_priority = 0;
  SAFE_SYSCALL(sched_setscheduler(0, SCHED_BATCH, &param));
  SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kFirstNiceness));
  observed_param.sched_priority = -1;
  EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_BATCH);
  SAFE_SYSCALL(sched_getparam(0, &observed_param));
  EXPECT_EQ(observed_param.sched_priority, 0);
  errno = 0;
  EXPECT_EQ(SAFE_SYSCALL(getpriority(PRIO_PROCESS, 0)), kFirstNiceness);
  EXPECT_EQ(errno, 0);

  param.sched_priority = 33;
  SAFE_SYSCALL(sched_setscheduler(0, SCHED_FIFO, &param));
  observed_param.sched_priority = -1;
  EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_FIFO);
  SAFE_SYSCALL(sched_getparam(0, &observed_param));
  EXPECT_EQ(observed_param.sched_priority, 33);

  SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kSecondNiceness));

  param.sched_priority = 35;
  SAFE_SYSCALL(sched_setscheduler(0, SCHED_RR, &param));
  observed_param.sched_priority = -1;
  EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_RR);
  SAFE_SYSCALL(sched_getparam(0, &observed_param));
  EXPECT_EQ(observed_param.sched_priority, 35);

  param.sched_priority = 0;
  SAFE_SYSCALL(sched_setscheduler(0, SCHED_OTHER, &param));
  EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_OTHER);
  EXPECT_EQ(SAFE_SYSCALL(getpriority(PRIO_PROCESS, 0)), kSecondNiceness);

  test_helper::ForkHelper().RunInForkedProcess([kFirstNiceness, kSecondNiceness]() {
    sched_param param{};
    sched_param observed_param{};

    param.sched_priority = 0;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_BATCH, &param));
    SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kFirstNiceness));
    observed_param.sched_priority = -1;
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_BATCH);
    SAFE_SYSCALL(sched_getparam(0, &observed_param));
    EXPECT_EQ(observed_param.sched_priority, 0);
    errno = 0;
    EXPECT_EQ(SAFE_SYSCALL(getpriority(PRIO_PROCESS, 0)), kFirstNiceness);
    EXPECT_EQ(errno, 0);

    param.sched_priority = 33;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_FIFO, &param));
    observed_param.sched_priority = -1;
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_FIFO);
    SAFE_SYSCALL(sched_getparam(0, &observed_param));
    EXPECT_EQ(observed_param.sched_priority, 33);

    SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kSecondNiceness));

    param.sched_priority = 35;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_RR, &param));
    observed_param.sched_priority = -1;
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_RR);
    SAFE_SYSCALL(sched_getparam(0, &observed_param));
    EXPECT_EQ(observed_param.sched_priority, 35);

    param.sched_priority = 0;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_OTHER, &param));
    EXPECT_EQ(SAFE_SYSCALL(sched_getscheduler(0)), SCHED_OTHER);
    EXPECT_EQ(SAFE_SYSCALL(getpriority(PRIO_PROCESS, 0)), kSecondNiceness);
  });
}

TEST(SuccessivePoliciesTest, UnusedNicenessIsStillSubjectToRLimit) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kNicenessRlimitCur = 11;
  constexpr int kNicenessRlimitMax = kNicenessRlimitCur + 3;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        sched_param param{};

        param.sched_priority = 0;
        SAFE_SYSCALL(sched_setscheduler(0, SCHED_IDLE, &param));
        SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, -20));

        rlimit niceness_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kNicenessRlimitCur),
            .rlim_max = static_cast<rlim_t>(kNicenessRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_NICE, &niceness_rlimit));

        param.sched_priority = 35;
        SAFE_SYSCALL(sched_setscheduler(0, SCHED_FIFO | SCHED_RESET_ON_FORK, &param));

        Become(kUser1Uid, kUser1Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    ready.hold();

    Become(kUser1Uid, kUser1Uid);

    // We can hold the niceness where it is:
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, -20), 0);

    // We can increase the niceness:
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, -19), 0);

    // We cannot decrease the niceness to a value disallowed by the rlimit:
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, -20), -1);
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), -19);

    // But the niceness can be adjusted within the rlimit-allowed range:
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, 19), 0);
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), 19);
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, 20 - kNicenessRlimitCur), 0);
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), 20 - kNicenessRlimitCur);
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, 19 - kNicenessRlimitCur), -1);
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), 20 - kNicenessRlimitCur);

    // All this is happening while the target is still using SCHED_FIFO
    // and making no use of the niceness.
    EXPECT_EQ(sched_getscheduler(target_pid), SCHED_FIFO | SCHED_RESET_ON_FORK);

    complete.poke();
  });
}

TEST(SuccessivePoliciesTest, ChangingPolicyWhenExceedingNicenessRLimitAllowedExceptOutOfIdle) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  // Not the min, not the max, otherwise arbitrary.
  constexpr int kRtpriorityRlimitCur = 47;
  constexpr int kRtpriorityRlimitMax = kRtpriorityRlimitCur + 5;
  constexpr int kNicenessRlimitCur = 11;
  constexpr int kNicenessRlimitMax = kNicenessRlimitCur + 23;

  test_helper::ForkHelper fork_helper;
  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous complete = test_helper::MakeRendezvous();

  pid_t target_pid =
      SpawnTarget(fork_helper, std::move(ready.poker), std::move(complete.holder), []() {
        rlimit rtpriority_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kRtpriorityRlimitCur),
            .rlim_max = static_cast<rlim_t>(kRtpriorityRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_RTPRIO, &rtpriority_rlimit));
        rlimit niceness_rlimit = {
            .rlim_cur = static_cast<rlim_t>(kNicenessRlimitCur),
            .rlim_max = static_cast<rlim_t>(kNicenessRlimitMax),
        };
        SAFE_SYSCALL(setrlimit(RLIMIT_NICE, &niceness_rlimit));

        // Set this process' niceness and priority to something
        // *beyond* what the rlimits allow.
        sched_param param = {.sched_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO))};
        SAFE_SYSCALL(sched_setscheduler(0, SCHED_FIFO, &param));
        EXPECT_EQ(setpriority(PRIO_PROCESS, 0, -10000), 0);

        Become(kUser1Uid, kUser1Uid);
      });

  fork_helper.RunInForkedProcess([ready = std::move(ready.holder),
                                  complete = std::move(complete.poker), target_pid]() mutable {
    Become(kUser1Uid, kUser1Uid);

    ready.hold();

    sched_param param{};
    int max_priority = SAFE_SYSCALL(sched_get_priority_max(SCHED_FIFO));

    // Shifting between real-time policies is allowed despite currently
    // having a priority above the RLIMIT_RTPRIO value.
    errno = 0;
    param.sched_priority = max_priority;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_RR, &param), 0);
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), 0);

    // Shifting to a non-real-time policy is allowed despite currently
    // having a niceness exceeding the RLIMIT_NICE value.
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_OTHER, &param), 0);

    // The shift didn't change the current exceeding-the-RLIMIT_NICE
    // niceness value.
    errno = 0;
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), -20);
    EXPECT_EQ(errno, 0);

    // Shifting among SCHED_OTHER and SCHED_BATCH is fine and doesn't
    // affect the exceeding-the-RLIMIT_NICE niceness.
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_BATCH, &param), 0);
    errno = 0;
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), -20);
    EXPECT_EQ(errno, 0);
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_OTHER, &param), 0);
    errno = 0;
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), -20);
    EXPECT_EQ(errno, 0);
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_BATCH, &param), 0);
    EXPECT_EQ(errno, 0);
    errno = 0;
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), -20);
    EXPECT_EQ(errno, 0);

    // We can shift back to a real-time policy, but in this case the
    // RLIMIT_RTPRIO does apply!
    param.sched_priority = kRtpriorityRlimitCur + 1;
    errno = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), -1);
    EXPECT_EQ(errno, EPERM);
    param.sched_priority = kRtpriorityRlimitCur;
    errno = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), 0);
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_RR, &param), 0);
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), 0);

    // SCHED_IDLE is something of a "trap" - see "Special rules apply
    // for the SCHED_IDLE policy [...] an unprivileged thread can switch
    // to either the SCHED_BATCH or the SCHED_OTHER policy so long as
    // its nice value falls within the range permitted by its RLIMIT_NICE
    // resource limit" at sched(7). So with the target niceness-rlimited,
    // we can transition it into SCHED_IDLE, but not from there into
    // either SCHED_BATCH or SCHED_OTHER:
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_IDLE, &param), 0);
    errno = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_OTHER, &param), -1);
    EXPECT_EQ(errno, EPERM);
    errno = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_BATCH, &param), -1);
    EXPECT_EQ(errno, EPERM);
    // Although sched(7) doesn't specifically address it, we see that we
    // also can't transition to either of the real-time policies:
    param.sched_priority = kRtpriorityRlimitCur;
    errno = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), -1);
    EXPECT_EQ(errno, EPERM);
    errno = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_RR, &param), -1);
    EXPECT_EQ(errno, EPERM);
    // But we can no-op "transition" from SCHED_IDLE to SCHED_IDLE:
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_IDLE, &param), 0);
    // To be able to get out of SCHED_IDLE, we bring the niceness into
    // the allowed range:
    EXPECT_EQ(setpriority(PRIO_PROCESS, target_pid, 20 - kNicenessRlimitCur), 0);
    // Now we can transition to any other policy.
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_OTHER, &param), 0);
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_IDLE, &param), 0);
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_BATCH, &param), 0);
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_IDLE, &param), 0);
    param.sched_priority = kRtpriorityRlimitCur;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_FIFO, &param), 0);
    param.sched_priority = 0;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_IDLE, &param), 0);
    param.sched_priority = kRtpriorityRlimitCur;
    EXPECT_EQ(sched_setscheduler(target_pid, SCHED_RR, &param), 0);

    complete.poke();
  });
}

TEST(SuccessivePoliciesTest, UnusedNegativeNicenessIsStillZeroedByResetOnFork) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    constexpr int kVeryUnniceNiceness = -18;

    sched_param param{};

    param.sched_priority = 0;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_IDLE, &param));
    SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kVeryUnniceNiceness));

    param.sched_priority = 35;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_FIFO | SCHED_RESET_ON_FORK, &param));

    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();
    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_EQ(sched_getscheduler(target_pid), SCHED_OTHER);
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), 0);

    // Meanwhile, this process' unused niceness hasn't changed.
    EXPECT_EQ(getpriority(PRIO_PROCESS, 0), kVeryUnniceNiceness);

    complete.poker.poke();
  });
}

// NOTE(nathaniel): The wording "If the calling process has a negative nice
// value, the nice value is reset to zero in child processes" in the specification
// of the reset-on-fork flag creates the impression that if the calling process'
// nice value is not negative, it is not reset to zero for children, but this
// does not turn out to be the case.
TEST(SuccessivePoliciesTest, UnusedPositiveNicenessIsZeroedDuringResetOnFork) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping test.";
  }

  test_helper::ForkHelper().RunInForkedProcess([]() {
    constexpr int kVeryNiceNiceness = 17;

    sched_param param{};

    param.sched_priority = 0;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_IDLE, &param));
    SAFE_SYSCALL(setpriority(PRIO_PROCESS, 0, kVeryNiceNiceness));

    param.sched_priority = 35;
    SAFE_SYSCALL(sched_setscheduler(0, SCHED_RR | SCHED_RESET_ON_FORK, &param));

    test_helper::ForkHelper target_process_fork_helper;
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();
    pid_t target_pid = target_process_fork_helper.RunInForkedProcess(
        [complete = std::move(complete.holder)]() mutable { complete.hold(); });

    EXPECT_EQ(sched_getscheduler(target_pid), SCHED_OTHER);
    EXPECT_EQ(getpriority(PRIO_PROCESS, target_pid), 0);

    // Meanwhile, this process' unused niceness hasn't changed.
    EXPECT_EQ(getpriority(PRIO_PROCESS, 0), kVeryNiceNiceness);

    complete.poker.poke();
  });
}

}  // namespace
