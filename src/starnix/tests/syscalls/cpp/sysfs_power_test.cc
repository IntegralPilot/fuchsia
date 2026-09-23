// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/stdcompat/string_view.h>
#include <sys/epoll.h>
#include <sys/poll.h>
#include <sys/timerfd.h>
#include <sys/uio.h>

#include <cstdint>
#include <format>
#include <regex>
#include <sstream>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

using testing::IsSupersetOf;

namespace {

bool ReadFileToStringNonBlocking(const std::string& path, std::string* content) {
  int fd = open(path.c_str(), O_RDONLY | O_NONBLOCK);
  if (fd < 0)
    return false;
  char buf[4096];
  ssize_t res = read(fd, buf, sizeof(buf) - 1);
  close(fd);
  if (res < 0)
    return false;
  buf[res] = '\0';
  *content = std::string(buf);
  return true;
}

void VerifyReadOutOfBound(const std::string& path) {
  constexpr int kOffset = 100;
  constexpr int kSize = 10;
  char read_buffer[kSize];
  struct iovec iov[] = {
      {
          .iov_base = &read_buffer[0],
          .iov_len = kSize,
      },
  };
  int fd = openat(AT_FDCWD, path.c_str(), O_RDONLY | O_NONBLOCK);
  EXPECT_NE(-1, fd);
  EXPECT_EQ(0, preadv(fd, iov, std::size(iov), kOffset)) << "preadv failed: " << errno;
}

bool attempt_suspend(const std::string& suspend_type) {
  GTEST_LOG_(INFO) << "attempt_suspend: " << suspend_type;
  return files::WriteFile("/sys/power/state", suspend_type);
}

timer_t start_interval_timer() {
  timer_t timer_id;
  struct sigevent sev = {};
  sev.sigev_notify = SIGEV_NONE;
  EXPECT_NE(-1, timer_create(CLOCK_REALTIME_ALARM, &sev, &timer_id));

  // Set a test timer that will trigger every 1 second to wake
  // up all the subsequent suspends.
  struct itimerspec its = {};
  its.it_value.tv_sec = 2;
  its.it_interval.tv_sec = 2;
  EXPECT_NE(-1, timer_settime(timer_id, 0, &its, nullptr));

  // TODO(https://fxbug.dev/373676361): This sleep is here to guarantee that
  // the hrtimer request has been sent before tests try to suspend.
  sleep(5);

  return timer_id;
}

void stop_interval_timer(timer_t timer_id) {
  struct itimerspec its = {};
  its.it_value.tv_sec = 0;
  its.it_interval.tv_sec = 0;
  EXPECT_NE(-1, timer_settime(timer_id, 0, &its, nullptr));
}

}  // namespace

class SysfsPowerTest : public ::testing::Test {
 public:
  void SetUp() override {
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    // Assume starnix always has /sys/power and /sys/kernel/wakeup_reasons
    if (!test_helper::IsStarnix() && access("/sys/power", F_OK) == -1 &&
        access("/sys/kernel/wakeup_reasons", F_OK) == -1) {
      GTEST_SKIP() << "/sys/power not available, skipping...";
    }
  }

  void TearDown() override {
    std::string wake_locks_str;
    if (files::ReadFileToString("/sys/power/wake_lock", &wake_locks_str)) {
      std::stringstream ss(wake_locks_str);
      std::string lock;
      while (ss >> lock) {
        files::WriteFile("/sys/power/wake_unlock", lock);
      }
    }
  }
};

TEST_F(SysfsPowerTest, PowerDirectoryContainsExpectedContents) {
  std::vector<std::string> files;
  EXPECT_TRUE(files::ReadDirContents("/sys/power", &files));
  EXPECT_THAT(files, IsSupersetOf({"suspend_stats", "wakeup_count", "state", "sync_on_suspend",
                                   "wake_lock", "wake_unlock"}));
}

TEST_F(SysfsPowerTest, WakeupReasonsDirectoryContainsExpectedContents) {
  std::vector<std::string> files;
  EXPECT_TRUE(files::ReadDirContents("/sys/kernel/wakeup_reasons", &files));
  EXPECT_THAT(files, IsSupersetOf({"last_resume_reason", "last_suspend_time"}));
}

TEST_F(SysfsPowerTest, SuspendStatsDirectoryContainsExpectedContents) {
  std::vector<std::string> suspend_stats_files;
  EXPECT_TRUE(files::ReadDirContents("/sys/power/suspend_stats", &suspend_stats_files));
  EXPECT_THAT(suspend_stats_files,
              IsSupersetOf({"success", "fail", "last_failed_dev", "last_failed_errno"}));
}

TEST_F(SysfsPowerTest, SuspendStatsFilesContainDefaultsSuccess) {
  std::string success_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/success", &success_str));
  EXPECT_TRUE(std::regex_match(success_str, std::regex("^[0-9]+\n")));
}

TEST_F(SysfsPowerTest, SuspendStatsFilesUpdateAfterSuccessfulSuspend) {
  std::string prev_success_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/success", &prev_success_str));
  std::string prev_fail_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/fail", &prev_fail_str));

  timer_t timer_id = start_interval_timer();
  ASSERT_TRUE(attempt_suspend("mem"));
  stop_interval_timer(timer_id);

  std::string new_success_str;
  std::string new_fail_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/success", &new_success_str));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/fail", &new_fail_str));
  EXPECT_EQ(std::atoi(prev_success_str.c_str()) + 1, std::atoi(new_success_str.c_str()));
  EXPECT_EQ(std::atoi(prev_fail_str.c_str()), std::atoi(new_fail_str.c_str()));
}

TEST_F(SysfsPowerTest, SuspendStatsFilesUpdateAfterFailedSuspend) {
  std::string prev_success_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/success", &prev_success_str));
  std::string prev_fail_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/fail", &prev_fail_str));

  fbl::unique_fd timer_fd(timerfd_create(CLOCK_REALTIME, 0));
  EXPECT_TRUE(timer_fd.is_valid());

  fbl::unique_fd epoll_fd(epoll_create(1));
  EXPECT_TRUE(epoll_fd.is_valid());

  struct epoll_event ev = epoll_event();
  ev.events = EPOLLIN | EPOLLWAKEUP;
  EXPECT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_ADD, timer_fd.get(), &ev));

  // Activate the EPOLLWAKEUP event to create the implicit wake lock.
  {
    struct itimerspec its = {};
    its.it_value.tv_sec = 1;
    EXPECT_EQ(0, timerfd_settime(timer_fd.get(), 0, &its, nullptr));

    int ret = 0;
    struct epoll_event out_ev;
    ret = epoll_wait(epoll_fd.get(), &out_ev, 1, -1);
    EXPECT_EQ(1, ret);

    uint64_t val = 0;
    // Read the event from the timer_fd to reset the pending events.
    EXPECT_EQ(1, read(timer_fd.get(), &val, 1));
  }

  // This should fail due to the implicit wake lock.
  ASSERT_FALSE(attempt_suspend("mem"));

  std::string new_success_str;
  std::string new_fail_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/success", &new_success_str));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/fail", &new_fail_str));
  EXPECT_EQ(std::atoi(prev_success_str.c_str()), std::atoi(new_success_str.c_str()));
  EXPECT_EQ(std::atoi(prev_fail_str.c_str()) + 1, std::atoi(new_fail_str.c_str()));

  // Wait on the events again, which should clear the EPOLLWAKEUP when
  // no events are returned.
  {
    int ret = 0;
    struct epoll_event out_ev;
    ret = epoll_wait(epoll_fd.get(), &out_ev, 1, 0);
    EXPECT_EQ(0, ret);
  }
}

TEST_F(SysfsPowerTest, SuspendStatsFilesContainDefaultsFail) {
  std::string fail_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/fail", &fail_str));
  EXPECT_TRUE(std::regex_match(fail_str, std::regex("^[0-9]+\n$")));
}

TEST_F(SysfsPowerTest, SuspendStatsFilesContainDefaultsLastFailedDev) {
  std::string last_failed_dev_str;
  EXPECT_TRUE(
      files::ReadFileToString("/sys/power/suspend_stats/last_failed_dev", &last_failed_dev_str));
  EXPECT_TRUE(std::regex_match(last_failed_dev_str, std::regex("^.*\n$")));
}

TEST_F(SysfsPowerTest, SuspendStatsFilesContainDefaultsLastFailedErrno) {
  std::string last_failed_errno_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/suspend_stats/last_failed_errno",
                                      &last_failed_errno_str));
  // These tests always run in an environment where suspends don't fail.
  std::regex pattern("^(0|-?([1-9]\\d*|\\d))\n$");
  EXPECT_TRUE(std::regex_match(last_failed_errno_str, pattern));
}

TEST_F(SysfsPowerTest, WakeupCountFileContainsExpectedContents) {
  std::string wakeup_count_str;
  // Use non-blocking read to avoid hanging if the system has active wake locks.
  EXPECT_TRUE(ReadFileToStringNonBlocking("/sys/power/wakeup_count", &wakeup_count_str));
  EXPECT_TRUE(std::regex_match(wakeup_count_str, std::regex("^[0-9]+\n$")));
}

TEST_F(SysfsPowerTest, WakeupCountFileReadOutofBound) {
  VerifyReadOutOfBound("/sys/power/wakeup_count");
}

TEST_F(SysfsPowerTest, WakeupCountFileWrite) {
  EXPECT_FALSE(files::WriteFile("/sys/power/wakeup_count", "test"));
  EXPECT_FALSE(files::WriteFile("/sys/power/wakeup_count", std::to_string(INT_MAX)));

  std::string wakeup_count_str;
  // Use non-blocking read to avoid hanging if the system has active wake locks.
  EXPECT_TRUE(ReadFileToStringNonBlocking("/sys/power/wakeup_count", &wakeup_count_str));
  EXPECT_TRUE(files::WriteFile("/sys/power/wakeup_count", wakeup_count_str));
}

TEST_F(SysfsPowerTest, SuspendStateFileContainsExpectedContents) {
  std::string states_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/state", &states_str));
  EXPECT_TRUE(std::regex_match(states_str, std::regex("([mem|freeze|disk|standby]\\s?)*\n$")));
}

TEST_F(SysfsPowerTest, SuspendStateFileReadOutOfBound) { VerifyReadOutOfBound("/sys/power/state"); }

TEST_F(SysfsPowerTest, SuspendStateFileWrite) {
  timer_t timer_id = start_interval_timer();

  ASSERT_TRUE(attempt_suspend("mem"));
  GTEST_LOG_(INFO) << "wake: 1";
  ASSERT_TRUE(attempt_suspend("mem\n"));
  GTEST_LOG_(INFO) << "wake: 2";
  ASSERT_TRUE(attempt_suspend("mem\nfoobar"));
  GTEST_LOG_(INFO) << "wake: 3";
  ASSERT_TRUE(attempt_suspend("freeze"));
  GTEST_LOG_(INFO) << "wake: 4";
  ASSERT_TRUE(attempt_suspend("freeze\n"));
  GTEST_LOG_(INFO) << "wake: 5";
  ASSERT_TRUE(attempt_suspend("freeze\nfoobar"));
  GTEST_LOG_(INFO) << "wake: 6";

  stop_interval_timer(timer_id);
}

TEST_F(SysfsPowerTest, SuspendStateFileWriteFailsWaitAgain) {
  fbl::unique_fd timer_fd(timerfd_create(CLOCK_REALTIME, 0));
  EXPECT_TRUE(timer_fd.is_valid());

  fbl::unique_fd epoll_fd(epoll_create(1));
  EXPECT_TRUE(epoll_fd.is_valid());

  struct epoll_event ev = epoll_event();
  ev.events = EPOLLIN | EPOLLWAKEUP;
  EXPECT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_ADD, timer_fd.get(), &ev));

  // Activate the EPOLLWAKEUP event to create the implicit wake lock.
  {
    struct itimerspec its = {};
    its.it_value.tv_sec = 1;
    EXPECT_EQ(0, timerfd_settime(timer_fd.get(), 0, &its, nullptr));

    int ret = 0;
    struct epoll_event out_ev;
    ret = epoll_wait(epoll_fd.get(), &out_ev, 1, -1);
    EXPECT_EQ(1, ret);

    uint64_t val = 0;
    // Read the event from the timer_fd to reset the pending events.
    EXPECT_EQ(1, read(timer_fd.get(), &val, 1));
  }

  // This should fail due to the implicit wake lock.
  ASSERT_FALSE(attempt_suspend("mem"));

  // Wait on the events again, which should clear the EPOLLWAKEUP when
  // no events are returned.
  {
    int ret = 0;
    struct epoll_event out_ev;
    ret = epoll_wait(epoll_fd.get(), &out_ev, 1, 0);
    EXPECT_EQ(0, ret);
  }

  // Attempt to suspend the system now that the implicit wake lock
  // should have been deleted.
  timer_t timer_id = start_interval_timer();
  ASSERT_TRUE(attempt_suspend("mem"));
  stop_interval_timer(timer_id);
}

TEST_F(SysfsPowerTest, SuspendStateFileWriteFailsCloseFD) {
  fbl::unique_fd timer_fd(timerfd_create(CLOCK_REALTIME, 0));
  EXPECT_TRUE(timer_fd.is_valid());

  fbl::unique_fd epoll_fd(epoll_create(1));
  EXPECT_TRUE(epoll_fd.is_valid());

  struct epoll_event ev = epoll_event();
  ev.events = EPOLLIN | EPOLLWAKEUP;
  EXPECT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_ADD, timer_fd.get(), &ev));

  // Activate the EPOLLWAKEUP event to create the implicit wake lock.
  {
    struct itimerspec its = {};
    its.it_value.tv_sec = 1;
    EXPECT_EQ(0, timerfd_settime(timer_fd.get(), 0, &its, nullptr));

    int ret = 0;
    struct epoll_event out_ev;
    ret = epoll_wait(epoll_fd.get(), &out_ev, 1, -1);
    EXPECT_EQ(1, ret);

    uint64_t val = 0;
    // Read the event from the timer_fd to reset the pending events.
    EXPECT_EQ(1, read(timer_fd.get(), &val, 1));
  }

  // This should fail due to the implicit wake lock.
  ASSERT_FALSE(attempt_suspend("mem"));

  // Closing the epoll file descriptor should remove the wake lock.
  epoll_fd.reset();

  // Attempt to suspend the system now that the implicit wake lock
  // should have been deleted.
  timer_t timer_id = start_interval_timer();
  ASSERT_TRUE(attempt_suspend("mem"));
  stop_interval_timer(timer_id);
}

TEST_F(SysfsPowerTest, SuspendStateFileWriteFailsCloseEpollFD) {
  fbl::unique_fd timer_fd(timerfd_create(CLOCK_REALTIME, 0));
  EXPECT_TRUE(timer_fd.is_valid());

  fbl::unique_fd epoll_fd(epoll_create(1));
  EXPECT_TRUE(epoll_fd.is_valid());

  struct epoll_event ev = epoll_event();
  ev.events = EPOLLIN | EPOLLWAKEUP;
  EXPECT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_ADD, timer_fd.get(), &ev));

  // Activate the EPOLLWAKEUP event to create the implicit wake lock.
  {
    struct itimerspec its = {};
    its.it_value.tv_sec = 1;
    EXPECT_EQ(0, timerfd_settime(timer_fd.get(), 0, &its, nullptr));

    int ret = 0;
    struct epoll_event out_ev;
    ret = epoll_wait(epoll_fd.get(), &out_ev, 1, -1);
    EXPECT_EQ(1, ret);

    uint64_t val = 0;
    // Read the event from the timer_fd to reset the pending events.
    EXPECT_EQ(1, read(timer_fd.get(), &val, 1));
  }

  // This should fail due to the implicit wake lock.
  ASSERT_FALSE(attempt_suspend("mem"));

  // Closing the epoll file descriptor should remove the wake lock.
  epoll_fd.reset();

  // Attempt to suspend the system now that the implicit wake lock
  // should have been deleted.
  timer_t timer_id = start_interval_timer();
  ASSERT_TRUE(attempt_suspend("mem"));
  stop_interval_timer(timer_id);
}

TEST_F(SysfsPowerTest, SuspendStateFileWriteFailsEpollDelete) {
  fbl::unique_fd timer_fd(timerfd_create(CLOCK_REALTIME, 0));
  EXPECT_TRUE(timer_fd.is_valid());

  fbl::unique_fd epoll_fd(epoll_create(1));
  EXPECT_TRUE(epoll_fd.is_valid());

  struct epoll_event ev = epoll_event();
  ev.events = EPOLLIN | EPOLLWAKEUP;
  EXPECT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_ADD, timer_fd.get(), &ev));

  // Activate the EPOLLWAKEUP event to create the implicit wake lock.
  {
    struct itimerspec its = {};
    its.it_value.tv_sec = 1;
    EXPECT_EQ(0, timerfd_settime(timer_fd.get(), 0, &its, nullptr));

    int ret = 0;
    struct epoll_event out_ev;
    ret = epoll_wait(epoll_fd.get(), &out_ev, 1, -1);
    EXPECT_EQ(1, ret);

    uint64_t val = 0;
    // Read the event from the timer_fd to reset the pending events.
    EXPECT_EQ(1, read(timer_fd.get(), &val, 1));
  }

  // This should fail due to the implicit wake lock.
  ASSERT_FALSE(attempt_suspend("mem"));

  // Deleting the epoll file descriptor should remove the wake lock.
  EXPECT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_DEL, timer_fd.get(), &ev));

  // Attempt to suspend the system now that the implicit wake lock
  // should have been deleted.
  timer_t timer_id = start_interval_timer();
  ASSERT_TRUE(attempt_suspend("mem"));
  stop_interval_timer(timer_id);
}

TEST_F(SysfsPowerTest, SuspendStateFileWriteInvalidFails) {
  ASSERT_FALSE(files::WriteFile("/sys/power/state", "test"));
  ASSERT_FALSE(files::WriteFile("/sys/power/state", "disk"));
}

TEST_F(SysfsPowerTest, SyncOnSuspendFileContainsExpectedContents) {
  std::string sync_on_suspend_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/power/sync_on_suspend", &sync_on_suspend_str));
  EXPECT_TRUE(std::regex_match(sync_on_suspend_str, std::regex("(0|1)\n")));
}

TEST_F(SysfsPowerTest, SyncOnSuspendFileReadOutOfBound) {
  VerifyReadOutOfBound("/sys/power/sync_on_suspend");
}

TEST_F(SysfsPowerTest, SyncOnSuspendFileWrite) {
  EXPECT_FALSE(files::WriteFile("/sys/power/sync_on_suspend", "test"));
  EXPECT_FALSE(files::WriteFile("/sys/power/sync_on_suspend", std::to_string(2)));
  EXPECT_TRUE(files::WriteFile("/sys/power/sync_on_suspend", std::to_string(0)));
}

TEST_F(SysfsPowerTest, LastSuspendTimeFileContainsExpectedContents) {
  std::string last_suspend_time_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/kernel/wakeup_reasons/last_suspend_time",
                                      &last_suspend_time_str));
  std::regex pattern("^(0|-?([1-9]\\d*|\\d)\\.\\d+)\\s(0|-?([1-9]\\d*|\\d)\\.\\d+)\n$");
  EXPECT_TRUE(std::regex_match(last_suspend_time_str, pattern));
}

TEST_F(SysfsPowerTest, LastResumeReasonFileContainsExpectedContents) {
  std::string last_suspend_time_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/kernel/wakeup_reasons/last_resume_reason",
                                      &last_suspend_time_str));
  EXPECT_TRUE(std::regex_match(last_suspend_time_str, std::regex("^.*\n$")));
}

TEST_F(SysfsPowerTest, AddAndRemoveWakeLock) {
  std::string wake_locks_str;
  std::string test_wake_lock_str = "test_wake_lock";
  EXPECT_TRUE(files::WriteFile("/sys/power/wake_lock", test_wake_lock_str));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_lock", &wake_locks_str));
  EXPECT_TRUE(std::regex_match(wake_locks_str, std::regex("^.*\n$")));
  EXPECT_TRUE(cpp23::contains(wake_locks_str, test_wake_lock_str));

  std::string wake_unlocks_str;
  EXPECT_TRUE(files::WriteFile("/sys/power/wake_unlock", test_wake_lock_str));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_unlock", &wake_unlocks_str));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_lock", &wake_locks_str));
  EXPECT_TRUE(std::regex_match(wake_unlocks_str, std::regex("^.*\n$")));
  EXPECT_TRUE(cpp23::contains(wake_unlocks_str, test_wake_lock_str));
  EXPECT_FALSE(cpp23::contains(wake_locks_str, test_wake_lock_str));
}

TEST_F(SysfsPowerTest, WakeLockBlocksSuspend) {
  std::string wake_locks_str;
  std::string test_wake_lock_str = "test_wake_lock";
  EXPECT_TRUE(files::WriteFile("/sys/power/wake_lock", test_wake_lock_str));

  // This should fail due to the implicit wake lock.
  ASSERT_FALSE(attempt_suspend("mem"));

  std::string wake_unlocks_str;
  EXPECT_TRUE(files::WriteFile("/sys/power/wake_unlock", test_wake_lock_str));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_unlock", &wake_unlocks_str));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_lock", &wake_locks_str));
  EXPECT_TRUE(cpp23::contains(wake_unlocks_str, test_wake_lock_str));
  EXPECT_FALSE(cpp23::contains(wake_locks_str, test_wake_lock_str));
}

TEST_F(SysfsPowerTest, WakeLockWithTimeout) {
  std::string wake_locks_str;
  std::string wake_unlocks_str;
  std::string test_wake_lock_str = "test_wake_lock";
  int wake_lock_timeout_s = 1;

  EXPECT_TRUE(files::WriteFile(
      "/sys/power/wake_lock",
      std::format("{} {}", test_wake_lock_str, wake_lock_timeout_s * 1000 * 1000 * 1000)));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_lock", &wake_locks_str));
  EXPECT_TRUE(cpp23::contains(wake_locks_str, test_wake_lock_str));

  // Wait until the wake lock timeout is expired
  sleep(wake_lock_timeout_s * 2);

  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_lock", &wake_locks_str));
  EXPECT_FALSE(cpp23::contains(wake_locks_str, test_wake_lock_str));
  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_unlock", &wake_unlocks_str));
  EXPECT_TRUE(cpp23::contains(wake_unlocks_str, test_wake_lock_str));
}

TEST_F(SysfsPowerTest, WakeLockFileWrite) {
  std::string wake_locks_str;

  EXPECT_TRUE(files::WriteFile("/sys/power/wake_lock", "test1"));
  EXPECT_TRUE(files::WriteFile("/sys/power/wake_lock", "test2\n"));
  EXPECT_TRUE(files::WriteFile("/sys/power/wake_lock", "test3 1000000000000000"));
  EXPECT_FALSE(files::WriteFile("/sys/power/wake_lock", "test4 "));
  EXPECT_FALSE(files::WriteFile("/sys/power/wake_lock", "test5 e"));

  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_lock", &wake_locks_str));

  EXPECT_TRUE(cpp23::contains(wake_locks_str, "test1"));
  EXPECT_TRUE(cpp23::contains(wake_locks_str, "test2"));
  EXPECT_TRUE(cpp23::contains(wake_locks_str, "test3"));
  EXPECT_FALSE(cpp23::contains(wake_locks_str, "test4"));
  EXPECT_FALSE(cpp23::contains(wake_locks_str, "test5"));

  EXPECT_TRUE(files::WriteFile("/sys/power/wake_unlock", "test1"));
  EXPECT_TRUE(files::WriteFile("/sys/power/wake_unlock", "test2"));
  EXPECT_TRUE(files::WriteFile("/sys/power/wake_unlock", "test3"));

  EXPECT_TRUE(files::ReadFileToString("/sys/power/wake_lock", &wake_locks_str));
  // Ensure there is no active wake lock.
  EXPECT_TRUE(std::ranges::all_of(wake_locks_str, isspace));
}

TEST_F(SysfsPowerTest, WakeupCountBehavior) {
  if (access("/sys/power/wakeup_count", F_OK) != 0) {
    GTEST_SKIP() << "/sys/power/wakeup_count not available, skipping...";
  }
  if (access("/sys/power/wake_lock", W_OK) != 0) {
    GTEST_SKIP() << "/sys/power/wake_lock not writable, skipping...";
  }

  const char* kLockName = "WakeupCountTestLock";

  // Open with O_NONBLOCK to check initial state
  int fd = open("/sys/power/wakeup_count", O_RDONLY | O_NONBLOCK);
  ASSERT_NE(fd, -1);

  char buf[64];
  ssize_t res = read(fd, buf, sizeof(buf) - 1);
  ASSERT_NE(res, -1);
  buf[res] = '\0';
  uint64_t initial_count = std::stoull(buf);

  // Acquire lock
  ASSERT_TRUE(files::WriteFile("/sys/power/wake_lock", kLockName));

  // Re-read should now fail with EAGAIN
  lseek(fd, 0, SEEK_SET);
  uint64_t val;
  EXPECT_EQ(read(fd, &val, sizeof(val)), -1);
  EXPECT_EQ(errno, EAGAIN);

  // Release lock
  ASSERT_TRUE(files::WriteFile("/sys/power/wake_unlock", kLockName));

  // Wait for readability (poll)
  struct pollfd pfd = {fd, POLLIN, 0};
  int poll_res = poll(&pfd, 1, 5000);
  EXPECT_EQ(poll_res, 1);
  EXPECT_TRUE(pfd.revents & POLLIN);

  // Read should succeed
  lseek(fd, 0, SEEK_SET);
  res = read(fd, buf, sizeof(buf) - 1);
  ASSERT_GT(res, 0);
  buf[res] = '\0';
  uint64_t final_count = std::stoull(buf);

  // Count should have incremented
  EXPECT_EQ(final_count, initial_count + 1);

  close(fd);
}
