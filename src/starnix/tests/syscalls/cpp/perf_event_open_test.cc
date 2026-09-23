// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <err.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <thread>

#include <gtest/gtest.h>
#include <linux/perf_event.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "test_helper.h"

namespace {

// From https://man7.org/linux/man-pages/man2/perf_event_open.2.html
struct read_format_data {
  uint64_t value;        /* The value of the event */
  uint64_t time_enabled; /* if PERF_FORMAT_TOTAL_TIME_ENABLED */
  uint64_t time_running; /* if PERF_FORMAT_TOTAL_TIME_RUNNING */
  uint64_t id;           /* if PERF_FORMAT_ID */
  uint64_t lost;         /* if PERF_FORMAT_LOST */
};

// We use this struct when we only request `time_running` in the `read_format`.
// Because the struct is order-dependent we don't want the `time_running` param to
// get written to `time_enabled`.
struct read_format_data_time_running {
  uint64_t value;
  uint64_t time_running;
};

struct read_format_data_id {
  uint64_t value;
  uint64_t id;
};

// Valid example inputs to use for tests when we aren't testing these values
// but still need to pass them in.
// TODO(https://fxbug.dev/409621963): implement permissions logic for any pid > 0.
const int32_t example_pid = 0;
const int32_t example_cpu = -1;  // Keep this as -1 for now so that it includes events on ANY CPU.
// TODO(https://fxbug.dev/409619971): handle cases other than -1.
const int example_group_fd = -1;
const long example_flags = 0;
perf_event_attr example_attr = {0, 0, 0, {}, 0, 0, 0};
const int32_t from_nanos = 1000000000;
// TODO(https://fxbug.dev/512482672): Don't use smaller duration,
// otherwise there might not be enough sample time.
const int sample_duration = 1000;  // 1000 ms
const int poll_duration = 250000;  // 250 ms
const int read_retries = 5;

// Returns an example perf_event_attr where none of the values matter
// except for the read_format, which is passed in.
perf_event_attr attr_with_read_format(uint64_t read_format) {
  return {PERF_TYPE_SOFTWARE, PERF_ATTR_SIZE_VER1, PERF_COUNT_SW_CPU_CLOCK, {}, 0, read_format, 0};
}

perf_event_attr attr_with_read_format(uint64_t read_format, uint64_t disabled) {
  return {PERF_TYPE_SOFTWARE,
          PERF_ATTR_SIZE_VER1,
          PERF_COUNT_SW_CPU_CLOCK,
          {},
          0,
          read_format,
          disabled};
}

// Write wrapper because there isn't one per the man7 page.
int32_t sys_perf_event_open(perf_event_attr* attr, int32_t pid, int32_t cpu, int group_fd,
                            unsigned long flags) {
  // Explicitly cast to int32_t because perf_event_open() returns `int`. Also, `FdNumber` is i32.
  return static_cast<int32_t>(syscall(__NR_perf_event_open, attr, pid, cpu, group_fd, flags));
}

TEST(PerfEventOpenTest, ValidInputsSucceed) {
  // The file descriptor value that is returned is not guaranteed to be a specific number.
  // Just check that's not -1 (error).
  // TODO(https://fxbug.dev/394960158): Change this test when we have something better
  // to test.
  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor = sys_perf_event_open(&example_attr, example_pid, example_cpu,
                                                  example_group_fd, example_flags);
    EXPECT_NE(file_descriptor, -1);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, InvalidPidAndCpuFails) {
  int32_t pid = -1;  // Invalid
  int32_t cpu = -1;  // Invalid

  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor =
        sys_perf_event_open(&example_attr, pid, cpu, example_group_fd, example_flags);
    EXPECT_THAT(file_descriptor, SyscallFailsWithErrno(EINVAL));
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, ReadEventWithTimeEnabledSucceeds) {
  uint64_t read_format = PERF_FORMAT_TOTAL_TIME_ENABLED;
  perf_event_attr attr = attr_with_read_format(read_format);

  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);
    EXPECT_NE(file_descriptor, -1);

    // read() on the file descriptor should return the number of bytes written to buffer,
    // and the buffer should contain read_format_data information for that event.
    char buffer[16];
    uint64_t read_length = syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    EXPECT_EQ(read_length, sizeof(buffer));

    read_format_data data;
    std::memcpy(&data, buffer, sizeof(buffer));

    // Check that the time_enabled param in secs is smaller than the current time.
    uint64_t time_enabled_secs = data.time_enabled / from_nanos;
    timespec current_time;
    clock_gettime(CLOCK_MONOTONIC, &current_time);
    uint64_t current_time_secs = current_time.tv_sec;

    EXPECT_LE(time_enabled_secs, current_time_secs);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, ReadEventWithTimeRunningSucceeds) {
  uint64_t read_format = PERF_FORMAT_TOTAL_TIME_RUNNING;
  perf_event_attr attr = attr_with_read_format(read_format);

  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);
    EXPECT_NE(file_descriptor, -1);

    // read() on the file descriptor should return the number of bytes written to buffer,
    // and the buffer should contain read_format_data information for that event.
    char buffer[16];
    uint64_t read_length = syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    EXPECT_EQ(read_length, sizeof(buffer));

    read_format_data_time_running data;
    std::memcpy(&data, buffer, sizeof(buffer));

    uint64_t time_running_secs = data.time_running / from_nanos;
    timespec current_time;
    clock_gettime(CLOCK_MONOTONIC, &current_time);
    uint64_t current_time_secs = current_time.tv_sec;

    EXPECT_LE(time_running_secs, current_time_secs);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, ReadEventWithPerfFormatIdSucceeds) {
  if (test_helper::HasSysAdmin()) {
    uint64_t read_format = PERF_FORMAT_ID;
    perf_event_attr attr = attr_with_read_format(read_format);
    uint64_t ids[3];

    for (uint64_t i = 0; i < 3; i++) {
      int32_t file_descriptor =
          sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);
      EXPECT_NE(file_descriptor, -1);

      char buffer[16];
      read_format_data_id data;
      syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
      std::memcpy(&data, buffer, sizeof(buffer));

      ids[i] = data.id;
      EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
    }

    EXPECT_LT(ids[0], ids[1]);
    EXPECT_LT(ids[1], ids[2]);
  }
}

TEST(PerfEventOpenTest, ReadEventWithTimeEnabledAndRunningSucceeds) {
  uint64_t read_format = PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING;
  perf_event_attr attr = attr_with_read_format(read_format);

  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);
    EXPECT_NE(file_descriptor, -1);

    char buffer[24];
    uint64_t read_length = syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    EXPECT_EQ(read_length, sizeof(buffer));

    read_format_data data;
    std::memcpy(&data, buffer, sizeof(buffer));

    // Check that the params are smaller than the current time.
    uint64_t time_enabled_secs = data.time_enabled / from_nanos;
    uint64_t time_running_secs = data.time_running / from_nanos;
    timespec current_time;
    clock_gettime(CLOCK_MONOTONIC, &current_time);
    uint64_t current_time_secs = current_time.tv_sec;

    EXPECT_EQ(time_enabled_secs, time_running_secs);
    EXPECT_LE(time_enabled_secs, current_time_secs);
    EXPECT_LE(time_running_secs, current_time_secs);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, ReadEventWithBufferTooSmallFails) {
  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor = sys_perf_event_open(&example_attr, example_pid, example_cpu,
                                                  example_group_fd, example_flags);
    EXPECT_NE(file_descriptor, -1);

    // Create buffer that is too small for the read() call to put stuff in. read() should
    // return ENOSPC.
    char buffer[7];
    EXPECT_THAT(syscall(__NR_read, file_descriptor, buffer, sizeof(buffer)),
                SyscallFailsWithErrno(ENOSPC));
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

// When disabled is passed in the initial attr params.
TEST(PerfEventOpenTest, WhenDisabledEventCountShouldBeZero) {
  if (test_helper::HasSysAdmin()) {
    perf_event_attr attr = {0, 0, 0, {}, 0, 0, 1};

    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);

    char buffer[8];
    uint64_t read_length = syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    EXPECT_EQ(read_length, sizeof(buffer));

    read_format_data data;
    std::memcpy(&data, buffer, sizeof(buffer));

    unsigned long count = 0;
    EXPECT_EQ(data.value, count);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

// When enabled is passed in the initial attr params.
TEST(PerfEventOpenTest, WhenEnabledEventCountShouldBeOne) {
  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor = sys_perf_event_open(&example_attr, example_pid, example_cpu,
                                                  example_group_fd, example_flags);

    char buffer[8];
    uint64_t read_length = syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    EXPECT_EQ(read_length, sizeof(buffer));

    read_format_data data;
    std::memcpy(&data, buffer, sizeof(buffer));

    EXPECT_GT(data.value, (uint64_t)0);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, SettingIoctlDisabledCallWorks) {
  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor = sys_perf_event_open(&example_attr, example_pid, example_cpu,
                                                  example_group_fd, example_flags);

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_DISABLE), -1);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, SettingIoctlEnabledCallWorks) {
  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor = sys_perf_event_open(&example_attr, example_pid, example_cpu,
                                                  example_group_fd, example_flags);
    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_ENABLE), -1);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, WhenResetAndDisabledEventCountShouldBeZero) {
  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor = sys_perf_event_open(&example_attr, example_pid, example_cpu,
                                                  example_group_fd, example_flags);

    char buffer[8];
    syscall(__NR_read, file_descriptor, buffer,
            sizeof(buffer));  // Read once: the test rf_value = 1
    uint64_t read_length =
        syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));  // Read twice: rf_value = 2
    EXPECT_EQ(read_length, sizeof(buffer));

    read_format_data data;
    std::memcpy(&data, buffer, sizeof(buffer));

    // Host test will return a real value, which is bigger.
    unsigned long count = 2;
    EXPECT_GE(data.value, count);

    // Disable and reset. Count value should now be 0 and stay there.
    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_DISABLE), -1);
    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_RESET), -1);
    count = 0;

    read_length = syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    EXPECT_EQ(read_length, sizeof(buffer));

    std::memcpy(&data, buffer, sizeof(buffer));

    EXPECT_EQ(data.value, count);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

// Example:
// - Start perf_event_open with disabled = 0 (enabled)
// - Do an event
// - Call IOC_DISABLE
// - Do a read() which will return a time_running (for that segment)
// - Do an event
// - Do a read() which will return the same time_running (because segment didn't change)
TEST(PerfEventOpenTest, WhenDisabledTimeRunningAndTimeEnabledAreCorrect) {
  uint64_t read_format = PERF_FORMAT_TOTAL_TIME_RUNNING | PERF_FORMAT_TOTAL_TIME_ENABLED;
  perf_event_attr attr = attr_with_read_format(read_format);

  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);

    char buffer[24];
    read_format_data data;

    printf("This is an event\n");
    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_DISABLE), -1);
    syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    std::memcpy(&data, buffer, sizeof(buffer));

    uint64_t time_running = data.time_running;
    uint64_t time_enabled = data.time_enabled;

    printf("This is an event\n");
    syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    std::memcpy(&data, buffer, sizeof(buffer));

    uint64_t time_running_after_disable = data.time_running;
    uint64_t time_enabled_after_disable = data.time_enabled;

    EXPECT_EQ(time_running, time_running_after_disable);
    EXPECT_EQ(time_enabled, time_enabled_after_disable);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

// Example:
// - Start perf_event_open with disabled = 0 (enabled)
// - Do an event
// - Do a read() which will return a time_running
// - Do an event
// - Do a read() which will return a larger time_running
TEST(PerfEventOpenTest, WhenEnabledTimeRunningIsCorrect) {
  uint64_t read_format = PERF_FORMAT_TOTAL_TIME_RUNNING;
  perf_event_attr attr = attr_with_read_format(read_format);

  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);
    char buffer[16];
    read_format_data_time_running data;

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_ENABLE), -1);
    printf("This is an event\n");
    syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    std::memcpy(&data, buffer, sizeof(buffer));
    uint64_t time_running = data.time_running;

    // Check that time later is bigger.
    printf("This is an event\n");
    syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    std::memcpy(&data, buffer, sizeof(buffer));
    uint64_t time_running_later = data.time_running;

    // This fails for the host test because time_running is always 0.
    // TODO(https://fxbug.dev/413146816): figure out what the real time_running is.
    EXPECT_LT(time_running, time_running_later);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

// Example:
// - Start perf_event_open with disabled = 0 (enabled)
// - Do an event
// - Do a read() which will return a time_running
// - Do an event
// - Do a read() which will return a larger time_running
TEST(PerfEventOpenTest, WhenEnabledTimeEnabledIsCorrect) {
  uint64_t read_format = PERF_FORMAT_TOTAL_TIME_ENABLED;
  perf_event_attr attr = attr_with_read_format(read_format);

  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);
    char buffer[16];
    read_format_data data;

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_ENABLE), -1);
    printf("This is an event\n");
    syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    std::memcpy(&data, buffer, sizeof(buffer));
    uint64_t time_enabled = data.time_enabled;

    // Check that time later is bigger.
    printf("This is an event\n");
    syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));
    std::memcpy(&data, buffer, sizeof(buffer));
    uint64_t time_enabled_later = data.time_enabled;

    EXPECT_LT(time_enabled, time_enabled_later);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

// Example:
// - Start perf_event_open with disabled = 1 (disabled)
// - Do an event
// - Do a read() which will return a time_running and time_enabled of 0 (ensures initialization)
TEST(PerfEventOpenTest, WhenDisabledTotalTimeRunningAndEnabledAreZero) {
  if (test_helper::HasSysAdmin()) {
    uint64_t read_format = PERF_FORMAT_TOTAL_TIME_RUNNING | PERF_FORMAT_TOTAL_TIME_ENABLED;
    perf_event_attr attr = attr_with_read_format(read_format, 1);
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);

    printf("This is an event\n");

    char buffer[24];
    syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));

    read_format_data data;
    std::memcpy(&data, buffer, sizeof(buffer));

    EXPECT_EQ(data.time_running, (uint64_t)0);
    EXPECT_EQ(data.time_enabled, (uint64_t)0);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

// Example:
// - Start perf_event_open with disabled = 1 (disabled)
// - Make multiple IOC_DISABLE calls
// - Do an event
// - Do a read() which will return a time_running of 0 (ensures it was initialized)
TEST(PerfEventOpenTest, MultipleDisablesDoesNotChangeTime) {
  if (test_helper::HasSysAdmin()) {
    uint64_t read_format = PERF_FORMAT_TOTAL_TIME_RUNNING;
    perf_event_attr attr = attr_with_read_format(read_format, 1);
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_DISABLE), -1);
    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_DISABLE), -1);

    printf("This is an event\n");

    char buffer[16];
    syscall(__NR_read, file_descriptor, buffer, sizeof(buffer));

    read_format_data_time_running data;
    std::memcpy(&data, buffer, sizeof(buffer));

    EXPECT_EQ(data.time_running, (uint64_t)0);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, ReadingFirstEightBytesCanReturnCountAsALong) {
  if (test_helper::HasSysAdmin()) {
    int32_t file_descriptor = sys_perf_event_open(&example_attr, example_pid, example_cpu,
                                                  example_group_fd, example_flags);

    // Both char buffer[8] (tested in previous tests) and long long (8 bytes) should work.
    long long count;
    syscall(__NR_read, file_descriptor, &count, sizeof(count));

    EXPECT_GE(count, 1);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

// Here is a full example of a counting case.
TEST(PerfEventOpenTest, CountingCPUClockSucceeds) {
  if (test_helper::HasSysAdmin()) {
    perf_event_attr attr;
    memset(&attr, 0, sizeof(attr));
    attr.type = PERF_TYPE_SOFTWARE;
    attr.size = sizeof(attr);
    attr.config = PERF_COUNT_SW_CPU_CLOCK;
    attr.disabled = 1;

    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, -1, example_group_fd, example_flags);
    EXPECT_NE(file_descriptor, -1);

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_RESET), -1);
    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_ENABLE), -1);

    printf("This is an event\n");

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_DISABLE), -1);

    long long count;
    syscall(__NR_read, file_descriptor, &count, sizeof(count));

    // TODO(https://fxbug.dev/402938671): this is expected to be 0 right now
    // because real value hasn't been implemented. When running on host, it will give
    // a real number (~6000-10000). Update this when we get a real instruction count.
    EXPECT_GT(count, -1);
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);
  }
}

/* Below are sampling tests */

// Valid attributes for sampling. On Linux for the first sampling event you'll get something like:
// perf_event_header { type = 9, size = 16, misc = 2 }.
perf_event_attr example_sampling_attr(uint64_t sample_type) {
  perf_event_attr attr;
  memset(&attr, 0, sizeof(attr));
  attr.type = PERF_TYPE_HARDWARE;
  attr.size = sizeof(attr);
  attr.config = PERF_COUNT_HW_INSTRUCTIONS;
  // Elects sampling instead of counting. Since right here one
  // event is 1 nanoseconds. It represent take one sample every
  // 250'000 nanoseconds
  attr.sample_period = 250'000;
  attr.sample_type = sample_type;
  attr.disabled = 1;      // Initiate as DISABLED as per Perfetto code use-case.
  attr.exclude_user = 0;  // Necessary otherwise we get zeros for sampling.
  attr.exclude_kernel = 1;
  attr.sample_id_all = 1;

  return attr;
}

// TODO(https://fxbug.dev/398914921): The Linux version of this test will fail because
// we are currently testing against semi-hardcoded values in the Starnix implementation.
// Use better EXPECT statements when we grab real values.
//
// We avoid checking the absolute value for sample id, because its value rely on a global
// counter that might be updated through other tests. We issue multiple perf_event_open()
// syscall in this test and validate sample_id got updated for different event group.
TEST(PerfEventOpenTest, SampleIdIsValid) {
  if (test_helper::HasSysAdmin()) {
    perf_event_attr attr_task_clock =
        example_sampling_attr(PERF_SAMPLE_IDENTIFIER | PERF_SAMPLE_ID);
    attr_task_clock.type = PERF_TYPE_SOFTWARE;
    attr_task_clock.config = PERF_COUNT_SW_TASK_CLOCK;

    perf_event_attr attr_cpu_clock = example_sampling_attr(PERF_SAMPLE_IDENTIFIER | PERF_SAMPLE_ID);
    attr_cpu_clock.type = PERF_TYPE_SOFTWARE;
    attr_cpu_clock.config = PERF_COUNT_SW_CPU_CLOCK;

    int32_t fd_task_1 = sys_perf_event_open(&attr_task_clock, example_pid, example_cpu,
                                            example_group_fd, example_flags);
    int32_t fd_task_2 =
        sys_perf_event_open(&attr_task_clock, example_pid, example_cpu, fd_task_1, example_flags);
    int32_t fd_cpu_1 = sys_perf_event_open(&attr_cpu_clock, example_pid, example_cpu,
                                           example_group_fd, example_flags);
    int32_t fd_cpu_2 =
        sys_perf_event_open(&attr_cpu_clock, example_pid, example_cpu, fd_cpu_1, example_flags);
    const uint32_t invalid = -1;

    auto get_sample_id = [](int32_t fd) {
      int num_pages = 2;
      size_t data_size = num_pages * getpagesize();
      size_t buffer_size = getpagesize() + data_size;
      void* address = mmap(nullptr, buffer_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
      EXPECT_NE(address, MAP_FAILED);

      EXPECT_NE(syscall(__NR_ioctl, fd, PERF_EVENT_IOC_ENABLE), -1);
      const int sample_duration_ms = 7000;  // 7s to collect samples
      printf("This is an event - start sampling for %u ms \n", sample_duration_ms);

      auto start = std::chrono::steady_clock::now();
      auto duration = std::chrono::milliseconds(sample_duration_ms);

      int x = 0;
      while (std::chrono::steady_clock::now() - start < duration) {
        // Busy loop to burn CPU and generate events
        x++;
        // Prevent the compiler from optimizing away this loop by pretending 'x'
        // is used by assembly and that memory might be modified.
        asm volatile("" : : "g"(x) : "memory");
      }
      EXPECT_NE(syscall(__NR_ioctl, fd, PERF_EVENT_IOC_DISABLE), -1);
      EXPECT_NE(syscall(__NR_close, fd), EXIT_FAILURE);

      uint64_t sample_id = invalid;
      uint64_t id = invalid;
      bool read_samples = false;
      int retries = 0;
      perf_event_mmap_page* metadata = (perf_event_mmap_page*)address;

      // Using __u64 to match the type in perf_event_mmap_page and avoid reference binding errors.
      std::atomic_ref<__u64> data_head(metadata->data_head);
      std::atomic_ref<__u64> data_tail(metadata->data_tail);

      while (!read_samples && retries < read_retries) {
        if (data_head.load(std::memory_order_acquire) <=
            data_tail.load(std::memory_order_acquire)) {
          usleep(poll_duration);
          retries += 1;
        } else {
          read_samples = true;
        }
      }

      uint64_t curr_pointer = data_tail.load(std::memory_order_acquire);

      while (curr_pointer < data_head.load(std::memory_order_acquire)) {
        char* record_start = static_cast<char*>(address) + metadata->data_offset + curr_pointer;

        // Verify that we got samples written by checking the first sample's metadata.
        perf_event_header* header = (perf_event_header*)record_start;

        if (header->size == 0) {
          break;
        }

        if (header->type == PERF_RECORD_SAMPLE) {
          // Verify the sample_id.
          char* record_details_start = record_start + sizeof(perf_event_header);
          struct perf_record_sample {
            uint64_t sample_id;
            uint64_t id;
          };
          struct perf_record_sample* record_details =
              (struct perf_record_sample*)record_details_start;
          sample_id = record_details->sample_id;
          id = record_details->id;
          EXPECT_EQ(sample_id, id);
        }

        curr_pointer += header->size;
      }

      EXPECT_TRUE(read_samples);
      EXPECT_EQ(curr_pointer, data_head.load(std::memory_order_relaxed));
      // Update data_tail to indicate we've consumed the records.
      data_tail.store(curr_pointer, std::memory_order_release);

      if (retries > 0) {
        printf("Retried reading sample data %u times\n", retries);
        // It's ok that we retried. However, if we retried several times and still didn't get
        // samples, something went wrong (e.g. profiler didn't start or stop correctly).
        EXPECT_TRUE(retries <= read_retries && read_samples);
      }
      EXPECT_EQ(syscall(__NR_munmap, address, buffer_size), 0);
      return sample_id;
    };

    uint64_t task_id_1 = get_sample_id(fd_task_1);
    uint64_t task_id_2 = get_sample_id(fd_task_2);
    uint64_t cpu_id_1 = get_sample_id(fd_cpu_1);
    uint64_t cpu_id_2 = get_sample_id(fd_cpu_2);

    EXPECT_NE(invalid, task_id_1);
    EXPECT_EQ(task_id_1, task_id_2);
    EXPECT_EQ(cpu_id_1, cpu_id_2);
    EXPECT_LT(task_id_1, cpu_id_1);
  }
}

TEST(PerfEventOpenTest, MmapMetadataPageIsValid) {
  if (test_helper::HasSysAdmin()) {
    perf_event_attr attr = example_sampling_attr(PERF_SAMPLE_TIME);
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);

    int num_pages = 2;
    size_t data_size = num_pages * getpagesize();
    // Ring buffer size, defined to be 1 + 2^n pages per the docs.
    size_t buffer_size = getpagesize() + data_size;

    // mmap() returns the address of the mapping. Note you MUST use MAP_SHARED because you're
    // doing kernel and user stuff. The offset has to be 0 to access the metadata page (first page).
    void* address = mmap(nullptr, buffer_size, PROT_READ, MAP_SHARED, file_descriptor, 0);

    // Address should not be 0xffffffffffffffff.
    EXPECT_NE(address, MAP_FAILED);

    // Docs say you can close here before reading anything.
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);

    char buffer[buffer_size];
    EXPECT_EQ(buffer_size, sizeof(buffer));

    // Verify metadata page has valid/reasonable info.
    // Don't need to memcopy. Just read directly.
    // Note: While we should technically use atomic loads for all shared memory
    // fields after mmap(), we use direct access here for simplicity since the
    // event is not enabled yet and values are expected to be static.
    perf_event_mmap_page* metadata = (perf_event_mmap_page*)address;
    EXPECT_LT(metadata->version, (uint32_t)10);
    EXPECT_LT(metadata->compat_version, (uint32_t)10);
    EXPECT_EQ(metadata->lock % 2, (uint32_t)0);
    EXPECT_NE(metadata->index, (uint32_t)-1);
    EXPECT_NE(metadata->offset, (int64_t)-1);
    EXPECT_EQ(metadata->capabilities, (uint64_t)30);
    EXPECT_EQ(metadata->cap_user_time, (uint64_t)1);
    EXPECT_EQ(metadata->time_enabled, (uint64_t)0);
    EXPECT_EQ(metadata->time_running, (uint64_t)0);
    EXPECT_EQ(metadata->data_head, (uint64_t)0);
    EXPECT_EQ(metadata->data_tail, (uint64_t)0);
    EXPECT_EQ(metadata->data_offset, (uint64_t)getpagesize());
    EXPECT_EQ(metadata->data_size, (uint64_t)data_size);
    EXPECT_EQ(syscall(__NR_munmap, address, buffer_size), 0);
  }
}

// TODO(https://fxbug.dev/398914921): The Linux version of this test will fail because
// we are currently testing against semi-hardcoded values in the Starnix implementation.
// Use better EXPECT statements when we grab real values.
TEST(PerfEventOpenTest, MmapFirstRecordPageIsValid) {
  if (test_helper::HasSysAdmin()) {
    perf_event_attr attr =
        example_sampling_attr(PERF_SAMPLE_IDENTIFIER | PERF_SAMPLE_IP | PERF_SAMPLE_TID |
                              PERF_SAMPLE_ID | PERF_SAMPLE_CALLCHAIN | PERF_SAMPLE_PERIOD);
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);

    // TODO(https://fxbug.dev/513339352): choose more reasonable num_pages.
    int num_pages = 256;
    size_t data_size = num_pages * getpagesize();
    // Ring buffer size, defined to be 1 + 2^n pages per the docs.
    size_t buffer_size = getpagesize() + data_size;

    // mmap() returns the address of the mapping. Note you MUST use MAP_SHARED because you're
    // doing kernel and user stuff.
    // The offset has to be 0 to access the metadata page (first page).
    void* address = mmap(nullptr, buffer_size, PROT_WRITE, MAP_SHARED, file_descriptor, 0);
    // Address should not be 0xffffffffffffffff.
    EXPECT_NE(address, MAP_FAILED);

    char buffer[buffer_size];
    EXPECT_EQ(buffer_size, sizeof(buffer));

    // Verify metadata page has valid/reasonable info.
    // Don't need to memcopy. Just read directly.
    // Note: While we should technically use atomic loads for all shared memory
    // fields after mmap(), we use direct access here for simplicity since the
    // event is not enabled yet and values are expected to be static.
    perf_event_mmap_page* metadata = (perf_event_mmap_page*)address;
    EXPECT_LT(metadata->version, (uint32_t)10);
    EXPECT_LT(metadata->compat_version, (uint32_t)10);
    EXPECT_EQ(metadata->lock % 2, (uint32_t)0);
    EXPECT_NE(metadata->index, (uint32_t)-1);
    EXPECT_NE(metadata->offset, (int64_t)-1);
    EXPECT_EQ(metadata->capabilities, (uint64_t)30);
    EXPECT_EQ(metadata->cap_user_time, (uint64_t)1);
    EXPECT_EQ(metadata->time_enabled, (uint64_t)0);
    EXPECT_EQ(metadata->time_running, (uint64_t)0);
    EXPECT_EQ(metadata->data_head, (uint64_t)0);
    EXPECT_EQ(metadata->data_tail, (uint64_t)0);
    EXPECT_EQ(metadata->data_offset, (uint64_t)getpagesize());
    EXPECT_EQ(metadata->data_size, data_size);

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_ENABLE), -1);
    // We are profiling the Starnix Kernel's job (not the test container's job).
    // We must make syscalls (e.g. getpid()) to generate activity inside the kernel job,
    // otherwise we will not get any samples.
    auto start = std::chrono::steady_clock::now();
    auto duration = std::chrono::milliseconds(sample_duration);
    while (std::chrono::steady_clock::now() - start < duration) {
      syscall(__NR_getpid);
    }
    // End sampling.
    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_DISABLE), -1);
    // As mentioned in the docs, closing the file descriptor does not invalidate
    // the mmap() mapping.
    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);

    // Start polling every 250 ms to read sampling data. If after 5 retries nothing
    // has been read, return that sampling has failed.
    // 250 ms poll is taken from
    // https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/perfetto/src/profiling/perf/event_config_unittest.cc;l=96
    bool read_samples = false;
    int retries = 0;

    // Using __u64 to match the type in perf_event_mmap_page and avoid reference binding errors.
    std::atomic_ref<__u64> data_head(metadata->data_head);
    std::atomic_ref<__u64> data_tail(metadata->data_tail);

    while (!read_samples && retries < read_retries) {
      // Start reading next page, which is the first sampling data page. From there
      // you can keep iterating to read each sample. Layout:
      //
      // The whole object is a Record, comprised of a Header and a RecordDetails:
      // ------           <-- record_start, start of the header and the sample.
      // |  |
      // |  |                 perf_event_header size (always 8 bytes)
      // |  |
      // |  ---           <-- record_details_start, start of the record_details.
      // |  |
      // |  |                 varying size based on `perf_event_header->type`
      // |  |
      // |  |
      // ------

      // If no sampling data, wait poll_duration to potentially collect data and retry.
      if (!read_samples) {
        if (data_head.load(std::memory_order_acquire) <=
            data_tail.load(std::memory_order_acquire)) {
          usleep(poll_duration);
          retries += 1;
          continue;
        } else {
          read_samples = true;
        }
      }

      // This is the counter that gets incremented after reading each record.
      uint64_t curr_pointer = data_tail.load(std::memory_order_acquire);

      while (curr_pointer < data_head.load(std::memory_order_acquire)) {
        // Starting address for the current record.
        char* record_start = static_cast<char*>(address) + metadata->data_offset + curr_pointer;

        // Parse header.
        perf_event_header* header = (perf_event_header*)record_start;

        if (header->size == 0) {
          break;
        }

        if (header->type == PERF_RECORD_SAMPLE) {
          EXPECT_THAT(header->misc, testing::AnyOf(testing::Eq(PERF_RECORD_MISC_KERNEL) /* 1 */,
                                                   testing::Eq(PERF_RECORD_MISC_USER) /* 2 */));
          EXPECT_GE(header->size, (uint16_t)8);  // Size of the whole sample, INCLUDING THIS HEADER.

          // Parse record details.
          char* record_details_start = record_start + sizeof(perf_event_header);

          // This is a subset of the real perf_record_sample which we will implement later.
          struct perf_record_sample {
            uint64_t sample_id;
            uint64_t ip;
            uint32_t pid;
            uint32_t tid;
            uint64_t id;
            uint64_t sample_period;
            uint64_t nr;
          };

          struct perf_record_sample* record_details =
              (struct perf_record_sample*)record_details_start;
          EXPECT_GE(record_details->ip, (uint64_t)1);
          EXPECT_GE(record_details->pid, (uint64_t)0);
          EXPECT_GE(record_details->tid, (uint64_t)1);
          EXPECT_EQ(record_details->id, record_details->sample_id);
          EXPECT_GE(record_details->sample_period, (uint64_t)250'000);
          // We expect some nesting of at least 1, to maybe 200, for getpid().
          EXPECT_GE(record_details->nr, (uint64_t)1);
          EXPECT_LT(record_details->nr, (uint64_t)200);

          // Read the next param of perf_record_sample ips[nr]. When we created the
          // struct, we don't know the size of `ips`. So we iterate over `nr` times.
          uint64_t number_of_ips = record_details->nr;
          uint64_t* ips_start =
              reinterpret_cast<uint64_t*>(record_details_start + sizeof(perf_record_sample));
          std::span<uint64_t> ips{ips_start, static_cast<std::size_t>(number_of_ips)};
          for (size_t i = 0; i < ips.size(); ++i) {
            uint64_t ip = ips[i];
            if (i == 0) {
              EXPECT_EQ(ip, record_details->ip);
            } else {
              // The profiler may return 0 for frames at the end of the stack or when walking fails.
              // We don't enforce non-zero for these frames.
            }
          }
        }

        // Advance counter.
        curr_pointer += header->size;
      }

      EXPECT_EQ(curr_pointer, data_head.load(std::memory_order_relaxed));
      // Update data_tail to indicate we've consumed the records.
      data_tail.store(curr_pointer, std::memory_order_release);

      EXPECT_TRUE(read_samples);
      if (read_samples) {
        EXPECT_GT(curr_pointer, (uint64_t)0);
        EXPECT_GT(data_head.load(std::memory_order_relaxed), (uint64_t)0);
      }
    }
    if (retries > 0) {
      printf("Retried reading sample data %u times\n", retries);
      // It's ok that we retried. However, if we retried several times and still didn't get
      // samples, something went wrong (e.g. profiler didn't start or stop correctly).
      EXPECT_TRUE(retries <= read_retries && read_samples);
    }
    EXPECT_GT(data_head.load(std::memory_order_acquire), (uint64_t)0);
    EXPECT_EQ(syscall(__NR_munmap, address, buffer_size), 0);
  }
}

TEST(PerfEventOpenTest, ThreadSamplingResolvesLinuxPidTid) {
  if (test_helper::HasSysAdmin()) {
    pid_t expected_pid = getpid();
    std::atomic<pid_t> expected_tid = 0;
    std::atomic<bool> stop_thread = false;

    std::thread worker([&]() {
      expected_tid.store(static_cast<pid_t>(syscall(__NR_gettid)), std::memory_order_release);
      while (!stop_thread.load(std::memory_order_relaxed)) {
        // Repeatedly issue a cheap syscall to keep the thread busy on CPU so the profiler samples
        // it.
        syscall(__NR_getpid);
      }
    });

    while (expected_tid.load(std::memory_order_acquire) == 0) {
      std::this_thread::yield();
    }

    auto join_worker = [&]() {
      stop_thread.store(true, std::memory_order_release);
      if (worker.joinable()) {
        worker.join();
      }
    };

    perf_event_attr attr = example_sampling_attr(PERF_SAMPLE_IP | PERF_SAMPLE_TID |
                                                 PERF_SAMPLE_CALLCHAIN | PERF_SAMPLE_PERIOD);
    int32_t file_descriptor =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);
    if (file_descriptor == -1) {
      join_worker();
      FAIL() << "sys_perf_event_open failed: " << strerror(errno);
    }

    int num_pages = 256;
    size_t data_size = num_pages * getpagesize();
    size_t buffer_size = getpagesize() + data_size;

    void* address =
        mmap(nullptr, buffer_size, PROT_READ | PROT_WRITE, MAP_SHARED, file_descriptor, 0);
    if (address == MAP_FAILED) {
      join_worker();
      syscall(__NR_close, file_descriptor);
      FAIL() << "mmap failed: " << strerror(errno);
    }

    perf_event_mmap_page* metadata = static_cast<perf_event_mmap_page*>(address);

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_ENABLE), -1);

    auto start = std::chrono::steady_clock::now();
    auto duration = std::chrono::milliseconds(sample_duration);
    while (std::chrono::steady_clock::now() - start < duration) {
      syscall(__NR_getpid);
    }

    EXPECT_NE(syscall(__NR_ioctl, file_descriptor, PERF_EVENT_IOC_DISABLE), -1);

    join_worker();

    EXPECT_NE(syscall(__NR_close, file_descriptor), EXIT_FAILURE);

    bool read_samples = false;
    int retries = 0;
    uint32_t matched_samples = 0;

    std::atomic_ref<__u64> data_head(metadata->data_head);
    std::atomic_ref<__u64> data_tail(metadata->data_tail);

    while (!read_samples && retries < read_retries) {
      if (!read_samples) {
        if (data_head.load(std::memory_order_acquire) <=
            data_tail.load(std::memory_order_acquire)) {
          usleep(poll_duration);
          retries += 1;
          continue;
        } else {
          read_samples = true;
        }
      }

      uint64_t curr_pointer = data_tail.load(std::memory_order_acquire);

      while (curr_pointer < data_head.load(std::memory_order_acquire)) {
        char* record_start = static_cast<char*>(address) + metadata->data_offset + curr_pointer;

        perf_event_header* header = reinterpret_cast<perf_event_header*>(record_start);

        if (header->size == 0) {
          break;
        }

        if (header->type == PERF_RECORD_SAMPLE) {
          EXPECT_THAT(header->misc, testing::AnyOf(testing::Eq(PERF_RECORD_MISC_KERNEL),
                                                   testing::Eq(PERF_RECORD_MISC_USER)));
          EXPECT_GE(header->size, static_cast<uint16_t>(8));

          char* record_details_start = record_start + sizeof(perf_event_header);

          struct perf_record_sample {
            uint64_t ip;
            uint32_t pid;
            uint32_t tid;
            uint64_t sample_period;
            uint64_t nr;
          };

          struct perf_record_sample* record_details =
              reinterpret_cast<struct perf_record_sample*>(record_details_start);
          EXPECT_GE(record_details->ip, static_cast<uint64_t>(1));
          EXPECT_GE(record_details->sample_period, static_cast<uint64_t>(250'000));
          EXPECT_GE(record_details->nr, static_cast<uint64_t>(1));
          EXPECT_LT(record_details->nr, static_cast<uint64_t>(200));

          if (record_details->pid == static_cast<uint32_t>(expected_pid) &&
              record_details->tid == static_cast<uint32_t>(expected_tid.load())) {
            matched_samples++;
          }
        }

        curr_pointer += header->size;
      }

      EXPECT_EQ(curr_pointer, data_head.load(std::memory_order_relaxed));
      data_tail.store(curr_pointer, std::memory_order_release);
    }

    if (retries > 0) {
      printf("Retried reading sample data %u times\n", retries);
      EXPECT_TRUE(retries <= read_retries && read_samples);
    }

    EXPECT_GT(matched_samples, static_cast<uint32_t>(0));
    EXPECT_GT(data_head.load(std::memory_order_acquire), static_cast<uint64_t>(0));
    EXPECT_EQ(syscall(__NR_munmap, address, buffer_size), 0);
  }
}

TEST(PerfEventOpenTest, InvalidDataTailIgnoredByKernel) {
  if (test_helper::HasSysAdmin()) {
    perf_event_attr attr = example_sampling_attr(PERF_SAMPLE_TIME);
    int32_t fd =
        sys_perf_event_open(&attr, example_pid, example_cpu, example_group_fd, example_flags);
    EXPECT_NE(fd, -1);

    // TODO(https://fxbug.dev/513339352): choose more reasonable num_pages.
    int num_pages = 256;
    size_t buffer_size = getpagesize() + (num_pages * getpagesize());
    void* address = mmap(nullptr, buffer_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    EXPECT_NE(address, MAP_FAILED);

    perf_event_mmap_page* metadata = (perf_event_mmap_page*)address;
    std::atomic_ref<__u64> data_head(metadata->data_head);
    std::atomic_ref<__u64> data_tail(metadata->data_tail);

    // Verify data_head is 0.
    EXPECT_EQ(data_head.load(), (uint64_t)0);

    // User sets an invalid data_tail (data_tail > data_head) that underflows
    // and wraps in circular masking space, leaving enough free space for writes.
    // data_size = num_pages * getpagesize() = 256 * 4096 = 1048576.
    // Setting tail to data_size - 4096 results in used_space = data_size - 4096
    // and free_space = 4096, which allows writes to succeed.
    uint64_t data_size = num_pages * getpagesize();
    data_tail.store(data_size - 4096, std::memory_order_release);

    // Start sampling.
    EXPECT_NE(syscall(__NR_ioctl, fd, PERF_EVENT_IOC_ENABLE), -1);
    // We are profiling the Starnix Kernel's job (not the test container's job).
    // We must make syscalls (e.g. getpid()) to generate activity inside the kernel job,
    // otherwise we will not get any samples.
    auto start = std::chrono::steady_clock::now();
    auto duration = std::chrono::milliseconds(sample_duration);
    while (std::chrono::steady_clock::now() - start < duration) {
      syscall(__NR_getpid);
    }
    EXPECT_NE(syscall(__NR_ioctl, fd, PERF_EVENT_IOC_DISABLE), -1);

    // Verify that the invalid data_tail was ignored, the kernel still wrote
    // samples (data_head > 0) and that the kernel did not modify the invalid data_tail.
    EXPECT_GT(data_head.load(std::memory_order_acquire), (uint64_t)0);
    EXPECT_EQ(data_tail.load(std::memory_order_acquire), data_size - 4096);

    EXPECT_EQ(syscall(__NR_munmap, address, buffer_size), 0);
    EXPECT_NE(syscall(__NR_close, fd), EXIT_FAILURE);
  }
}

TEST(PerfEventOpenTest, GroupLeaderCleanup) {
  if (test_helper::HasSysAdmin()) {
    perf_event_attr attr_a = {};
    attr_a.type = PERF_TYPE_SOFTWARE;
    attr_a.config = PERF_COUNT_SW_CPU_CLOCK;
    attr_a.disabled = 1;

    perf_event_attr attr_b = {};
    attr_b.type = PERF_TYPE_SOFTWARE;
    attr_b.config = PERF_COUNT_SW_TASK_CLOCK;
    attr_b.disabled = 1;

    // Create group A.
    int fd_a = sys_perf_event_open(&attr_a, example_pid, example_cpu, -1, 0);
    EXPECT_NE(fd_a, -1);

    // Create group B.
    int fd_b = sys_perf_event_open(&attr_b, example_pid, example_cpu, -1, 0);
    EXPECT_NE(fd_b, -1);

    // Add an event to group A.
    int fd_a2 = sys_perf_event_open(&attr_a, example_pid, example_cpu, fd_a, 0);
    EXPECT_NE(fd_a2, -1);

    // Close the leader of group A.
    EXPECT_NE(syscall(__NR_close, fd_a), EXIT_FAILURE);

    // Try to add another event to group A. This should fail.
    EXPECT_THAT(sys_perf_event_open(&attr_a, example_pid, example_cpu, fd_a, 0),
                SyscallFailsWithErrno(EBADF));

    // Add an event to group B. This should succeed.
    int fd_b2 = sys_perf_event_open(&attr_b, example_pid, example_cpu, fd_b, 0);
    EXPECT_NE(fd_b2, -1);

    // Cleanup.
    EXPECT_NE(syscall(__NR_close, fd_a2), EXIT_FAILURE);
    EXPECT_NE(syscall(__NR_close, fd_b), EXIT_FAILURE);
    EXPECT_NE(syscall(__NR_close, fd_b2), EXIT_FAILURE);
  }
}

}  // namespace
