// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <cerrno>
#include <cstdint>
#include <memory>
#include <string>

#include <gtest/gtest.h>
#include <linux/capability.h>    /* Definition of _LINUX_CAPABILITY_VERSION_3 */
#include <linux/hw_breakpoint.h> /* Definition of HW_* constants */
#include <linux/perf_event.h>    /* Definition of PERF_* constants */

#include "fbl/unique_fd.h"
#include "gmock/gmock.h"
#include "src/lib/files/file.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

uint64_t valid_tracepoint_id = 1;

extern std::string DoPrePolicyLoadWork() {
  // Mount the tracefs to a temporary directory to be able to read the tracepoint IDs.
  test_helper::ScopedTempDir tracing_dir;
  if (mount("tracefs", tracing_dir.path().c_str(), "tracefs", 0, nullptr) == 0) {
    std::string id_path = tracing_dir.path() + "/events/sched/sched_switch/id";
    FILE* f = fopen(id_path.c_str(), "r");
    if (f) {
      if (fscanf(f, "%lu", &valid_tracepoint_id) != 1) {
        valid_tracepoint_id = 1;
      }
      fclose(f);
    }
  }
  return "perf_event_policy";
}

namespace {

constexpr int kAllTasksPid = -1;
constexpr int kCurrentTaskPid = 0;

int perf_event_open(struct perf_event_attr* attr, pid_t pid, int cpu) {
  return (int)syscall(__NR_perf_event_open, attr, pid, cpu, /*group_fd=*/-1, /*flags=*/0);
}

class PerfEventTest : public ::testing::Test {};

std::unique_ptr<struct perf_event_attr> GetPerfEventAttr(uint32_t type, uint64_t config,
                                                         bool exclude_kernel) {
  auto pe = std::make_unique<struct perf_event_attr>();

  // Set parameters for the event we want to measure:
  pe->type = type;
  pe->size = sizeof(struct perf_event_attr);
  pe->config = config;

  // Include or exclude count for events that happen in kernel space.
  // When `0` the `kernel` permission is always checked.
  pe->exclude_kernel = exclude_kernel;

  return pe;
}

uint64_t GetConfigForPerfType(uint32_t perf_type) {
  return perf_type == PERF_TYPE_TRACEPOINT ? valid_tracepoint_id : 1;
}

class OpenTypeHardware : public PerfEventTest,
                         public testing::WithParamInterface<std::tuple<int, int>> {};

TEST_P(OpenTypeHardware, OpenEventsAllPermissions) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping HW counter tests on Linux";
  }
  /// When all permissions are provided, perf_event_open with type Hardware succeeds.
  auto enforce = ScopedEnforcement::SetEnforcing();
  auto& [config, pid] = GetParam();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_all_permissions_t:s0", [&] {
    auto pe = GetPerfEventAttr(PERF_TYPE_HARDWARE, config, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), pid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST_P(OpenTypeHardware, OpenEventsNoKernel) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping HW counter tests on Linux";
  }
  /// When exclude_kernel is 1, perf_event_open doesn't require the kernel permission.
  auto enforce = ScopedEnforcement::SetEnforcing();
  auto& [config, pid] = GetParam();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_kernel_t:s0", [&] {
    auto pe = GetPerfEventAttr(PERF_TYPE_HARDWARE, config, /*exclude_kernel=*/true);
    fbl::unique_fd fd(perf_event_open(pe.get(), pid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST_P(OpenTypeHardware, OpenEventsNoKernelFails) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping HW counter tests on Linux";
  }
  /// When exclude_kernel is 0, and kernel permission is missing, expect a failure.
  auto enforce = ScopedEnforcement::SetEnforcing();
  auto& [config, pid] = GetParam();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_kernel_t:s0", [&] {
    auto pe = GetPerfEventAttr(PERF_TYPE_HARDWARE, config,
                               /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), pid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
  }));
}

// Missing values: PERF_COUNT_HW_BUS_CYCLES, PERF_COUNT_HW_REF_CPU_CYCLES
// Because those fail with "No such file or directory".
const auto kPerfEventOpenHardware = ::testing::Combine(
    ::testing::Values(PERF_COUNT_HW_CPU_CYCLES, PERF_COUNT_HW_INSTRUCTIONS,
                      PERF_COUNT_HW_CACHE_REFERENCES, PERF_COUNT_HW_CACHE_MISSES,
                      PERF_COUNT_HW_BRANCH_INSTRUCTIONS, PERF_COUNT_HW_BRANCH_MISSES,
                      PERF_COUNT_HW_STALLED_CYCLES_FRONTEND, PERF_COUNT_HW_STALLED_CYCLES_BACKEND),
    ::testing::Values(kAllTasksPid, kCurrentTaskPid));
INSTANTIATE_TEST_SUITE_P(PerfEventTest, OpenTypeHardware, kPerfEventOpenHardware);

class OpenTypeSoftware : public PerfEventTest,
                         public testing::WithParamInterface<std::tuple<int, int>> {};

TEST_P(OpenTypeSoftware, OpenEventsAllPermissions) {
  /// When all permissions are provided, perf_event_open with type Software succeeds.
  auto enforce = ScopedEnforcement::SetEnforcing();
  auto& [config, pid] = GetParam();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_all_permissions_t:s0", [&] {
    auto pe = GetPerfEventAttr(PERF_TYPE_SOFTWARE, config, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), pid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST_P(OpenTypeSoftware, OpenEventsNoKernel) {
  /// When exclude_kernel is 1, perf_event_open doesn't require the kernel permission.
  auto enforce = ScopedEnforcement::SetEnforcing();
  auto& [config, pid] = GetParam();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_kernel_t:s0", [&] {
    auto pe = GetPerfEventAttr(PERF_TYPE_SOFTWARE, config, /*exclude_kernel=*/true);
    fbl::unique_fd fd(perf_event_open(pe.get(), pid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST_P(OpenTypeSoftware, OpenEventsNoKernelFails) {
  /// When exclude_kernel is 0, and kernel permission is missing, expect a failure.
  auto enforce = ScopedEnforcement::SetEnforcing();
  auto& [config, pid] = GetParam();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_kernel_t:s0", [&] {
    auto pe = GetPerfEventAttr(PERF_TYPE_SOFTWARE, config, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), pid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
  }));
}

// All correct values of config for PERF_EVENT_TYPE_SOFTWARE.
const auto kPerfEventOpenSoftware = ::testing::Combine(
    ::testing::Range(0, (int)PERF_COUNT_SW_MAX), ::testing::Values(kAllTasksPid, kCurrentTaskPid));
INSTANTIATE_TEST_SUITE_P(PerfEventTest, OpenTypeSoftware, kPerfEventOpenSoftware);

class OpenTypeHWCacheOrTracepoint : public PerfEventTest,
                                    public testing::WithParamInterface<uint32_t> {};

TEST_P(OpenTypeHWCacheOrTracepoint, OpenEventsAllPermissions) {
  // perf_event_open succeeds over different values of `pid` and `exclude_kernel`.
  const auto perf_type = OpenTypeHWCacheOrTracepoint::GetParam();
  if (perf_type == PERF_TYPE_HW_CACHE && !test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping HW cache tests on Linux";
  }
  const auto config = GetConfigForPerfType(perf_type);

  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_all_permissions_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // All `pid`s, exclude_kernel = 1.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 0.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/false);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 1.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST_P(OpenTypeHWCacheOrTracepoint, OpenEventsNoKernel) {
  // Without the kernel permission, perf_event_open only succeeds when `exclude_kernel` == 1.
  const auto perf_type = OpenTypeHWCacheOrTracepoint::GetParam();
  if (perf_type == PERF_TYPE_HW_CACHE && !test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping HW cache tests on Linux";
  }
  const auto config = GetConfigForPerfType(perf_type);

  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_kernel_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // All `pid`s, exclude_kernel = 1.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 0.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/false);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // Current pid, exclude_kernel = 1.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST_P(OpenTypeHWCacheOrTracepoint, OpenEventsNoCpu) {
  // Without the CPU permission, perf_event_open only succeeds when `pid` == 0.
  const auto perf_type = OpenTypeHWCacheOrTracepoint::GetParam();
  if (perf_type == PERF_TYPE_HW_CACHE && !test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping HW cache tests on Linux";
  }
  const auto config = GetConfigForPerfType(perf_type);

  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_cpu_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // All `pid`s, exclude_kernel = 1.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // Current pid, exclude_kernel = 0.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/false);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 1.
    pe = GetPerfEventAttr(perf_type, config, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

const auto kPerfEventOpenHWCacheOrTracepoint =
    ::testing::Values(PERF_TYPE_HW_CACHE, PERF_TYPE_TRACEPOINT);
INSTANTIATE_TEST_SUITE_P(PerfEventTest, OpenTypeHWCacheOrTracepoint,
                         kPerfEventOpenHWCacheOrTracepoint);

TEST(PerfEventTest, OpenEventsNoTracepoint) {
  // perf_event_open fails for PERF_TYPE_TRACEPOINT without the tracepoint permission.
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_tracepoint_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // All `pid`s, exclude_kernel = 1.
    pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 0.
    pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/false);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 1.
    pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, OpenEventsNoTracepointNoPerfmon) {
  // perf_event_open fails for PERF_TYPE_TRACEPOINT without the tracepoint permission.
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_tracepoint_no_perfmon_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // All `pid`s, exclude_kernel = 1.
    pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // Current pid, exclude_kernel = 0.
    pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/false);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // Current pid, exclude_kernel = 1.
    pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/true);
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, OpenEventsNoPerfmon) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_perfmon_t:s0", [&] {
    auto pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/true);
    fbl::unique_fd fd(perf_event_open(pe.get(), -1 /* All tasks */, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(PerfEventTest, OpenEventsNoPerfmonWithSysAdmin) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_perfmon_with_sys_admin_t:s0", [&] {
    auto pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, valid_tracepoint_id, /*exclude_kernel=*/true);
    fbl::unique_fd fd(perf_event_open(pe.get(), -1 /* All tasks */, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, OpenEventsBreakpoint) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  int watched_variable = 0;
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_all_permissions_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(PERF_TYPE_BREAKPOINT, 1, /*exclude_kernel=*/false);
    pe->bp_type = HW_BREAKPOINT_W;
    pe->bp_addr = (uint64_t)&watched_variable;
    pe->bp_len = sizeof(int);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // All `pid`s, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 0.
    pe->exclude_kernel = 0;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, OpenEventsBreakpointNoKernel) {
  // perf_event_open with type Breakpoint fails when exclude_kernel == 1 and kernel permission is
  // missing.
  auto enforce = ScopedEnforcement::SetEnforcing();
  int watched_variable = 0;
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_kernel_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(PERF_TYPE_BREAKPOINT, 1, /*exclude_kernel=*/false);
    pe->bp_type = HW_BREAKPOINT_W;
    pe->bp_addr = (uint64_t)&watched_variable;
    pe->bp_len = sizeof(int);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // All `pid`s, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 0.
    pe->exclude_kernel = 0;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // Current pid, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, OpenEventsBreakpointNoCpu) {
  // perf_event_open with type Breakpoint fails when pid == -1 and cpu permission is
  // missing.
  auto enforce = ScopedEnforcement::SetEnforcing();
  int watched_variable = 0;
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_cpu_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(PERF_TYPE_BREAKPOINT, 1, /*exclude_kernel=*/false);
    pe->bp_type = HW_BREAKPOINT_W;
    pe->bp_addr = (uint64_t)&watched_variable;
    pe->bp_len = sizeof(int);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // All `pid`s, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // Current pid, exclude_kernel = 0.
    pe->exclude_kernel = 0;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, OpenEventsRaw) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping Raw event tests on Linux";
  }
  auto enforce = ScopedEnforcement::SetEnforcing();
  // Define the raw event code (Event 0x24, Umask 0x41, combined)
  auto RAW_L2_RQSTS_EVENT = 0x4124;
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_all_permissions_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(PERF_TYPE_RAW, RAW_L2_RQSTS_EVENT, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // All `pid`s, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 0.
    pe->exclude_kernel = 0;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, OpenEventsRawNoKernel) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping Raw event tests on Linux";
  }
  auto enforce = ScopedEnforcement::SetEnforcing();
  // Define the raw event code (Event 0x24, Umask 0x41, combined)
  auto RAW_L2_RQSTS_EVENT = 0x4124;
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_kernel_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(PERF_TYPE_RAW, RAW_L2_RQSTS_EVENT, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // All `pid`s, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 0.
    pe->exclude_kernel = 0;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // Current pid, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, OpenEventsRawNoCpu) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "Skipping Raw event tests on Linux";
  }
  auto enforce = ScopedEnforcement::SetEnforcing();
  // Define the raw event code (Event 0x24, Umask 0x41, combined)
  auto RAW_L2_RQSTS_EVENT = 0x4124;
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_cpu_t:s0", [&] {
    // All `pid`s, exclude_kernel = 0.
    auto pe = GetPerfEventAttr(PERF_TYPE_RAW, RAW_L2_RQSTS_EVENT, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // All `pid`s, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));

    // Current pid, exclude_kernel = 0.
    pe->exclude_kernel = 0;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Current pid, exclude_kernel = 1.
    pe->exclude_kernel = 1;
    fd = fbl::unique_fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, ReadWriteEventsAllPermissions) {
  // When all permissions are provided, perf_event_open and then read succeed.
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_all_permissions_t:s0", [&] {
    auto pe =
        GetPerfEventAttr(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_RESET, 0), SyscallSucceeds());
    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_ENABLE, 0), SyscallSucceeds());
    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_DISABLE, 0), SyscallSucceeds());

    uint64_t count;
    EXPECT_THAT(read(fd.get(), &count, sizeof(count)), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, ReadEventsNoReadPermission) {
  // When the read permission is missing, perf_event_open succeeds but read fails.
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_read_t:s0", [&] {
    const size_t map_size = 2 * sysconf(_SC_PAGESIZE);

    auto pe =
        GetPerfEventAttr(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // There are no read checks for the ioctl syscall.
    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_RESET, 0), SyscallSucceeds());
    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_ENABLE, 0), SyscallSucceeds());
    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_DISABLE, 0), SyscallSucceeds());

    // Both read and mmap attempts should fail due to lack of read access.
    uint64_t count;
    EXPECT_THAT(read(fd.get(), &count, sizeof(count)), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT((uint64_t)mmap(nullptr, map_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(PerfEventTest, WriteEventsNoWritePermission) {
  // When the write permission is missing, perf_event_open succeeds but ioctl fails.
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_write_t:s0", [&] {
    const size_t map_size = 2 * sysconf(_SC_PAGESIZE);

    auto pe =
        GetPerfEventAttr(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // Write checks should fail on the ioctl syscall.
    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_RESET, 0), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_ENABLE, 0), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(ioctl(fd.get(), PERF_EVENT_IOC_DISABLE, 0), SyscallFailsWithErrno(EACCES));

    // The read check should succeed.
    uint64_t count;
    EXPECT_THAT(read(fd.get(), &count, sizeof(count)), SyscallSucceeds());
    EXPECT_THAT((uint64_t)mmap(nullptr, map_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0),
                SyscallSucceeds());
  }));
}

TEST(PerfEventTest, MmapInvalidArgumentsCheckOrder) {
  // Calling mmap with invalid arguments, while also having no read permissions, should fail with
  // EINVAL instead of EACCES. I.e. the arguments are checked before performing the security checks.
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_read_t:s0", [&] {
    auto pe =
        GetPerfEventAttr(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());

    // read should fail with EACCES.
    uint64_t count;
    EXPECT_THAT(read(fd.get(), &count, sizeof(count)), SyscallFailsWithErrno(EACCES));
    // mmap fails with EINVAL, instead of EACCES.
    EXPECT_THAT((uint64_t)mmap(nullptr, 0, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0),
                SyscallFailsWithErrno(EINVAL));
  }));
}

TEST(PerfEventTest, InvalidTracepointIdAndNoPermission) {
  // This test checks that perf_event_open fails with EINVAL for non existent tracepoint IDs
  // while lacking the `tracepoint` permission.
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_tracepoint_t:s0", [&] {
    // Use a bogus tracepoint ID.
    auto pe = GetPerfEventAttr(PERF_TYPE_TRACEPOINT, 0x7FFFFFFF, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kAllTasksPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EINVAL));
  }));
}

TEST(PerfEventTest, OpenEventsWithSelinuxButNoCapabilityFails) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_all_permissions_t:s0", [&] {
    test_helper::DropAllCapabilities();

    auto pe =
        GetPerfEventAttr(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(PerfEventTest, PermissiveOpenEventsNoPermissions) {
  /// Checks the order of checking the different permission types.
  auto enforce = ScopedEnforcement::SetPermissive();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_permissions_t:s0", [&] {
    auto pe =
        GetPerfEventAttr(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallSucceeds());
  }));
}

TEST(PerfEventTest, PermissiveOpenEventsNoCapabilities) {
  auto enforce = ScopedEnforcement::SetPermissive();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_perf_event_no_permissions_t:s0", [&] {
    test_helper::DropAllCapabilities();

    auto pe =
        GetPerfEventAttr(PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_CLOCK, /*exclude_kernel=*/false);
    fbl::unique_fd fd(perf_event_open(pe.get(), kCurrentTaskPid, 0 /* this CPU */));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace
