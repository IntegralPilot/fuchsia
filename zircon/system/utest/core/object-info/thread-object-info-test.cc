// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/clock.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/object.h>

#include <thread>

#include <zxtest/zxtest.h>

#include "helper.h"

namespace object_info_test {
namespace {

constexpr auto job_provider = []() -> const zx::job& {
  static const zx::unowned_job job = zx::job::default_job();
  return *job;
};

constexpr auto process_provider = []() -> const zx::process& {
  static const zx::unowned_process process = zx::process::self();
  return *process;
};

constexpr auto thread_provider = []() -> const zx::thread& {
  const static zx::unowned_thread thread = zx::thread::self();
  return *thread;
};

TEST(ThreadGetInfoTest, InfoHandleBasicOnSelfSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckSelfInfoSucceeds<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, thread_provider())));
}

TEST(ThreadGetInfoTest, InfoHandleBasicInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidHandleFails<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleBasicNullAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckNullAvailSucceeds<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleBasicNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckNullActualSucceeds<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleBasicNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE((CheckNullActualAndAvailSucceeds<zx_info_handle_basic_t>(
      ZX_INFO_HANDLE_BASIC, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleBasicInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE((CheckInvalidBufferPointerFails<zx_info_handle_basic_t>(
      ZX_INFO_HANDLE_BASIC, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleBasicBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      (BadActualIsInvalidArgs<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleBasicBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      (BadAvailIsInvalidArgs<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleBasicZeroSizedBufferFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckZeroSizeBufferFails<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleCountOnSelfSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckSelfInfoSucceeds<zx_info_handle_count_t>(ZX_INFO_HANDLE_COUNT, thread_provider())));
}

TEST(ThreadGetInfoTest, InfoHandleCountInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidHandleFails<zx_info_handle_count_t>(ZX_INFO_HANDLE_COUNT, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleCountNullAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckNullAvailSucceeds<zx_info_handle_count_t>(ZX_INFO_HANDLE_COUNT, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleCountNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckNullActualSucceeds<zx_info_handle_count_t>(ZX_INFO_HANDLE_COUNT, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleCountNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE((CheckNullActualAndAvailSucceeds<zx_info_handle_count_t>(
      ZX_INFO_HANDLE_COUNT, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleCountInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE((CheckInvalidBufferPointerFails<zx_info_handle_count_t>(
      ZX_INFO_HANDLE_COUNT, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleCountBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      (BadActualIsInvalidArgs<zx_info_handle_count_t>(ZX_INFO_HANDLE_COUNT, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleCountBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      (BadAvailIsInvalidArgs<zx_info_handle_count_t>(ZX_INFO_HANDLE_COUNT, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoHandleCountZeroSizedBufferFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckZeroSizeBufferFails<zx_info_handle_count_t>(ZX_INFO_HANDLE_COUNT, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadOnSelfSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckSelfInfoSucceeds<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider())));
}

TEST(ThreadGetInfoTest, InfoThreadInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidHandleFails<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadNullAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckNullAvailSucceeds<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckNullActualSucceeds<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckNullActualAndAvailSucceeds<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidBufferPointerFails<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      (BadActualIsInvalidArgs<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      (BadAvailIsInvalidArgs<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadZeroSizedBufferFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckZeroSizeBufferFails<zx_info_thread_t>(ZX_INFO_THREAD, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadJobHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckWrongHandleTypeFails<zx_info_thread_t, 32>(ZX_INFO_THREAD, job_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadProcessHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckWrongHandleTypeFails<zx_info_thread_t, 32>(ZX_INFO_THREAD, process_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadStatsOnSelfSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckSelfInfoSucceeds<zx_info_thread_stats_t>(ZX_INFO_THREAD_STATS, thread_provider())));
}

TEST(ThreadGetInfoTest, InfoThreadStatsInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidHandleFails<zx_info_thread_stats_t>(ZX_INFO_THREAD_STATS, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadStatsNullAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullAvailSucceeds<zx_info_thread_stats_t>(ZX_INFO_THREAD_STATS, thread_provider));
}

TEST(ThreadGetInfoTest, InfoThreadStatsNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualSucceeds<zx_info_thread_stats_t>(ZX_INFO_THREAD_STATS, thread_provider));
}

TEST(ThreadGetInfoTest, InfoThreadStatsNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(CheckNullActualAndAvailSucceeds<zx_info_thread_stats_t>(
      ZX_INFO_THREAD_STATS, thread_provider));
}

TEST(ThreadGetInfoTest, InfoThreadStatsInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE((CheckInvalidBufferPointerFails<zx_info_thread_stats_t>(
      ZX_INFO_THREAD_STATS, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadStatsBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      BadActualIsInvalidArgs<zx_info_thread_stats_t>(ZX_INFO_THREAD_STATS, thread_provider));
}

TEST(ThreadGetInfoTest, InfoThreadStatsBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      BadAvailIsInvalidArgs<zx_info_thread_stats_t>(ZX_INFO_THREAD_STATS, thread_provider));
}

TEST(ThreadGetInfoTest, InfoThreadStatsZeroSizedBufferFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckZeroSizeBufferFails<zx_info_thread_stats_t>(ZX_INFO_THREAD_STATS, thread_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadStatsJobHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckWrongHandleTypeFails<zx_info_thread_stats_t, 32>(ZX_INFO_THREAD_STATS, job_provider)));
}

TEST(ThreadGetInfoTest, InfoThreadStatsProcessHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE((CheckWrongHandleTypeFails<zx_info_thread_stats_t, 32>(
      ZX_INFO_THREAD_STATS, process_provider)));
}

// As reference from previous object-info test.
// Skip most tests for ZX_INFO_THREAD_EXCEPTION_REPORT, which is tested
// elsewhere and requires the target thread to be in a certain state.
TEST(ThreadGetInfoTest, InfoThreadExceptionReportInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE((CheckInvalidHandleFails<zx_exception_report_t, 32>(
      ZX_INFO_THREAD_EXCEPTION_REPORT, []() { return zx::handle(); })));
}

TEST(ThreadGetInfoTest, InfoThreadStatsRuntime) {
  // Spawn a thread to test runtime stats early in a thread's lifetime.
  std::thread test_thread{[&] {
    const zx::time begin_mono = zx::clock::get_monotonic();

    zx_info_thread_stats_t begin_thread;
    ASSERT_OK(zx::thread::self()->get_info(ZX_INFO_THREAD_STATS, &begin_thread,
                                           sizeof(begin_thread), nullptr, nullptr));

    ASSERT_OK(zx::nanosleep(zx::deadline_after(zx::msec(10))));

    zx_info_thread_stats_t end_thread;
    ASSERT_OK(zx::thread::self()->get_info(ZX_INFO_THREAD_STATS, &end_thread, sizeof(end_thread),
                                           nullptr, nullptr));

    const zx::time end_mono = zx::clock::get_monotonic();

    const zx::duration delta_mono = end_mono - begin_mono;
    const zx::duration delta_thread =
        zx::time{end_thread.total_runtime} - zx::time{begin_thread.total_runtime};

    EXPECT_GE(begin_mono.get(), 0);
    EXPECT_GE(end_mono.get(), 0);
    EXPECT_GE(begin_thread.total_runtime, 0);
    EXPECT_GE(end_thread.total_runtime, 0);
    EXPECT_GE(delta_mono.get(), 0);
    EXPECT_GE(delta_thread.get(), 0);
    EXPECT_GE((delta_mono - delta_thread).get(), 0);
  }};

  test_thread.join();
}

}  // namespace
}  // namespace object_info_test
