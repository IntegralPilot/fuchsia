// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>
#include <sys/time.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

TEST(TimeTest, ClockGetResMonotonic) {
  clockid_t clockid = CLOCK_MONOTONIC;
  struct timespec tp;
  ASSERT_THAT(clock_getres(clockid, &tp), SyscallSucceeds());
  ASSERT_EQ(tp.tv_sec, 0);
  ASSERT_EQ(tp.tv_nsec, 1);
}

TEST(TimeTest, ClockGetResRealTime) {
  clockid_t clockid = CLOCK_REALTIME;
  struct timespec tp;
  ASSERT_THAT(clock_getres(clockid, &tp), SyscallSucceeds());
  ASSERT_EQ(tp.tv_sec, 0);
  ASSERT_EQ(tp.tv_nsec, 1);
}

TEST(TimeTest, ClockGetResSyscallFail) {
  // Setting clockid to 15 as it will cause an error to be thrown.
  // The vDSO will check if the clockid is valid using the function is_valid_cpu_clock.
  // Since it isn't valid, the vDSO throws an error.
  clockid_t clockid = 15;
  struct timespec tp;
  tp.tv_nsec = 0;
  ASSERT_THAT(clock_getres(clockid, &tp), SyscallFails());
}

// TODO(https://fxbug.dev/350763590) remove no_sanitize attribute and diagnostic suppression
TEST(TimeTest, GetTimeOfDayNullTvSomeTz) __attribute__((no_sanitize("nonnull-attribute"))) {
  struct timezone tz;
// glibc adds nonnull attribute to the tv argument in getttimeofday.
// gettimeofday, however, does allow the tv argument to be NULL.
// To test that the vdso gettimeofday function allows tv to be NULL, the nonnull warning is
// temporarily disabled.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wnonnull"
  ASSERT_THAT(gettimeofday(nullptr, &tz), SyscallSucceeds());
#pragma GCC diagnostic pop
}

TEST(TimeTest, GetTimeOfDaySyscallVsVDSO) {
  struct timeval tv1, tv2, tv3;
  ASSERT_THAT(gettimeofday(&tv1, nullptr), SyscallSucceeds());
  ASSERT_THAT(syscall(SYS_gettimeofday, &tv2, nullptr), SyscallSucceeds());
  ASSERT_THAT(gettimeofday(&tv3, nullptr), SyscallSucceeds());
  EXPECT_LE(std::make_pair(tv1.tv_sec, tv1.tv_usec), std::make_pair(tv2.tv_sec, tv2.tv_usec));
  EXPECT_LE(std::make_pair(tv2.tv_sec, tv2.tv_usec), std::make_pair(tv3.tv_sec, tv3.tv_usec));
}

TEST(TimeTest, GetTimeOfDaySomeTvNullTz) {
  struct timeval tv;
  ASSERT_THAT(gettimeofday(&tv, nullptr), SyscallSucceeds());
}

TEST(TimeTest, GetTimeOfDaySomeTvSomeTz) {
  struct timeval tv;
  struct timezone tz;
  ASSERT_THAT(gettimeofday(&tv, &tz), SyscallSucceeds());
}

// TODO(https://fxbug.dev/350763590) remove no_sanitize attribute and diagnostic suppression
TEST(TimeTest, GetTimeOfDayNullTvNullTz) __attribute__((no_sanitize("nonnull-attribute"))) {
// glibc adds nonnull attribute to the tv argument in getttimeofday.
// gettimeofday, however, does allow the tv argument to be NULL.
// To test that the vdso gettimeofday function allows tv to be NULL, the nonnull warning is
// temporarily disabled.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wnonnull"
  ASSERT_THAT(gettimeofday(nullptr, nullptr), SyscallSucceeds());
#pragma GCC diagnostic pop
}

TEST(TimeTest, AdjustedTimeReflectedInVdso) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "AdjustedTimeReflectedInVdso test requires CAP_SYS_ADMIN";
  }

  // Get time via vDSO (standard gettimeofday)
  struct timeval tv_vdso1;
  ASSERT_EQ(0, gettimeofday(&tv_vdso1, nullptr));

  // Get time via syscall
  struct timeval tv_sys1;
  ASSERT_EQ(0, syscall(SYS_gettimeofday, &tv_sys1, nullptr));

  // Adjust time by winding it forward.
  struct timeval tv_adjust = tv_sys1;
  tv_adjust.tv_sec += 10000;

  // Use syscall to set time.
  ASSERT_EQ(0, syscall(SYS_settimeofday, &tv_adjust, nullptr)) << strerror(errno);

  // Get time via vDSO again.
  struct timeval tv_vdso2;
  ASSERT_EQ(0, gettimeofday(&tv_vdso2, nullptr));

  // Get time via syscall again.
  struct timeval tv_sys2;
  ASSERT_EQ(0, syscall(SYS_gettimeofday, &tv_sys2, nullptr));

  // Verify that the vDSO time has jumped forward.
  EXPECT_GE(tv_vdso2.tv_sec, tv_adjust.tv_sec);

  // Verify that vDSO and syscall agree (roughly).
  long diff = tv_sys2.tv_sec - tv_vdso2.tv_sec;
  if (diff < 0) {
    diff = -diff;
  }
  EXPECT_LE(diff, 1);
}
