// Copyright 2016 The Fuchsia Authors. All rights reserved.
//
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/captive-thread/captive-thread.h>
#include <lib/core-test-utils.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/clock.h>
#include <lib/zx/debuglog.h>
#include <lib/zx/event.h>
#include <lib/zx/handle.h>
#include <lib/zx/port.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmo.h>
#include <stddef.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/port.h>
#include <zircon/threads.h>
#include <zircon/types.h>

#include <atomic>
#include <limits>

#include <mini-process/mini-process.h>
#include <zxtest/zxtest.h>

#include "register-set.h"
#include "test-thread.h"
#include "thread-functions/thread-functions.h"

namespace {

// How far the PC must be advanced to skip over a breakpoint after it hits.
#if defined(__aarch64__)
constexpr int kBreakpointPcAdjustment = 4;
#elif defined(__riscv)
constexpr int kBreakpointPcAdjustment = 2;
#elif defined(__x86_64__)
constexpr int kBreakpointPcAdjustment = 0;
#endif

constexpr char kThreadName[] = "test-thread";

// We have to poll a thread's state as there is no way to wait for it to
// transition states. Wait this amount of time. Generally the thread won't
// take very long so this is a compromise between polling too frequently and
// waiting too long.
constexpr zx_duration_mono_t THREAD_BLOCKED_WAIT_DURATION = ZX_MSEC(1);

void get_koid(zx_handle_t handle, zx_koid_t* koid) {
  zx_info_handle_basic_t info;
  size_t records_read;
  ASSERT_EQ(
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), &records_read, nullptr),
      ZX_OK);
  ASSERT_EQ(records_read, 1u);
  *koid = info.koid;
}

bool get_thread_info(zx_handle_t thread, zx_info_thread_t* info) {
  return zx_object_get_info(thread, ZX_INFO_THREAD, info, sizeof(*info), nullptr, nullptr) == ZX_OK;
}

// Suspend the given thread and block until it reaches the suspended state. The suspend token
// is written to the output parameter.
void suspend_thread_synchronous(zx_handle_t thread, zx_handle_t* suspend_token) {
  ASSERT_EQ(zx_task_suspend(thread, suspend_token), ZX_OK);

  zx_signals_t observed = 0u;
  ASSERT_EQ(zx_object_wait_one(thread, ZX_THREAD_SUSPENDED, ZX_TIME_INFINITE, &observed), ZX_OK);
}

// Resume the given thread and block until it reaches the running state.
void resume_thread_synchronous(zx_handle_t thread, zx_handle_t suspend_token) {
  ASSERT_EQ(zx_handle_close(suspend_token), ZX_OK);

  zx_signals_t observed = 0u;
  ASSERT_EQ(zx_object_wait_one(thread, ZX_THREAD_RUNNING, ZX_TIME_INFINITE, &observed), ZX_OK);
}

// Updates the thread state to advance over a software breakpoint instruction, assuming the
// breakpoint was just hit. This does not resume the thread, only updates its state.
void advance_over_breakpoint(zx_handle_t thread) {
  if (kBreakpointPcAdjustment != 0) {
    // Advance to the next instruction after the debug break.
    zx_thread_state_general_regs_t regs;
    ASSERT_EQ(zx_thread_read_state(thread, ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)),
              ZX_OK);
    regs.REG_PC += kBreakpointPcAdjustment;
    ASSERT_EQ(zx_thread_write_state(thread, ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)),
              ZX_OK);
  }
}

// Waits for the exception type excp_type, ignoring exceptions of type ignore_type (these will
// just resume the thread), and issues errors for anything else.
//
// Fills |exception_out| with the resulting exception object.
void wait_thread_excp_type(zx_handle_t thread, zx_handle_t exception_channel, uint32_t excp_type,
                           uint32_t ignore_type, zx_handle_t* exception_out) {
  while (true) {
    ASSERT_EQ(zx_object_wait_one(exception_channel, ZX_CHANNEL_READABLE, ZX_TIME_INFINITE, nullptr),
              ZX_OK);

    zx_exception_info_t info;
    zx_handle_t exception;
    ASSERT_EQ(
        zx_channel_read(exception_channel, 0, &info, &exception, sizeof(info), 1, nullptr, nullptr),
        ZX_OK);

    // Use EXPECTs rather than ASSERTs here so that if something fails
    // we log all the relevant information about the packet contents.
    zx_koid_t koid = ZX_KOID_INVALID;
    get_koid(thread, &koid);
    EXPECT_EQ(info.tid, koid);

    if (info.type != ignore_type) {
      EXPECT_EQ(info.type, excp_type);
      *exception_out = exception;
      break;
    } else {
      uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
      ASSERT_EQ(zx_object_set_property(exception, ZX_PROP_EXCEPTION_STATE, &state, sizeof(state)),
                ZX_OK);
      zx_handle_close(exception);
    }
  }
}

// Wait until |thread| is in one of the specified |states|.
// We wait forever and let Unittest's watchdog handle errors.
void wait_thread_state(zx_handle_t thread, cpp20::span<const zx_thread_state_t> states) {
  while (true) {
    zx_info_thread_t info;
    ASSERT_TRUE(get_thread_info(thread, &info));
    for (auto s : states) {
      if (info.state == s) {
        return;
      }
    }
    zx_nanosleep(zx_deadline_after(THREAD_BLOCKED_WAIT_DURATION));
  }
}

// Wait for |thread| to enter blocked state |reason|.
// We wait forever and let Unittest's watchdog handle errors.
void wait_thread_blocked(zx_handle_t thread, zx_thread_state_t reason) {
  wait_thread_state(thread, cpp20::span(&reason, 1));
}

bool CpuMaskBitSet(const zx_cpu_set_t& set, uint32_t i) {
  if (i >= ZX_CPU_SET_MAX_CPUS) {
    return false;
  }
  uint32_t word = i / ZX_CPU_SET_BITS_PER_WORD;
  uint32_t bit = i % ZX_CPU_SET_BITS_PER_WORD;
  return ((set.mask[word] >> bit) & 1u) != 0;
}

TEST(Threads, Basics) {
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("Basics"));
  ASSERT_NO_FATAL_FAILURE(
      test_thread.Start(threads_test_sleep_fn, zx_deadline_after(ZX_MSEC(100))));
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

TEST(Threads, InvalidRights) {
  zx_handle_t thread;
  zx_handle_t ro_process_h;

  constexpr std::string_view kName = "test_thread";
  ASSERT_EQ(zx_handle_duplicate(zx_process_self(), ZX_RIGHT_DESTROY, &ro_process_h), ZX_OK);
  ASSERT_EQ(
      zx_thread_create(ro_process_h, kName.data(), static_cast<uint32_t>(kName.size()), 0, &thread),
      ZX_ERR_ACCESS_DENIED);

  ASSERT_EQ(zx_handle_close(ro_process_h), ZX_OK);
}

TEST(Threads, EmptyNameSucceeds) {
  zx_handle_t thread;
  ASSERT_EQ(zx_thread_create(zx_process_self(), "", 0, 0, &thread), ZX_OK);
  char thread_name[ZX_MAX_NAME_LEN] = {};
  ASSERT_EQ(zx_object_get_property(thread, ZX_PROP_NAME, thread_name, ZX_MAX_NAME_LEN), ZX_OK);
  ASSERT_EQ(strcmp(thread_name, ""), 0);
  ASSERT_EQ(zx_handle_close(thread), ZX_OK);
}

TEST(Threads, LongNameSucceeds) {
  // Creating a thread with a super long name should succeed.
  static constexpr std::string_view kLongName =
      "0123456789012345678901234567890123456789"
      "0123456789012345678901234567890123456789";
  ASSERT_GT(kLongName.size(), (size_t)ZX_MAX_NAME_LEN - 1, "too short to truncate");

  zx::thread thread;
  ASSERT_OK(zx::thread::create(*zx::process::self(), kLongName.data(),
                               static_cast<uint32_t>(kLongName.size()), 0, &thread));
  std::array<char, ZX_MAX_NAME_LEN> thread_name = {};
  ASSERT_OK(thread.get_property(ZX_PROP_NAME, thread_name.data(), thread_name.size()));
  ASSERT_EQ(thread_name.back(), '\0');
  ASSERT_EQ(memcmp(thread_name.data(), kLongName.data(), thread_name.size() - 1), 0);
}

// zx_thread_start() is supposed to be usable for creating a
// process's first thread.
TEST(Threads, ThreadStartOnInitialThread) {
  if (getenv("NO_NEW_PROCESS")) {
    ZXTEST_SKIP("Running without the ZX_POL_NEW_PROCESS policy, skipping test case.");
  }
  static constexpr char kProcessName[] = "test-proc-thread1";
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t thread;
  ASSERT_OK(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar));
  ASSERT_OK(zx_thread_create(process, kThreadName, sizeof(kThreadName) - 1, 0, &thread));
  ASSERT_EQ(ZX_OK, zx_thread_start(thread, 0, 1, 1, 1));

  ASSERT_OK(zx_handle_close(thread));
  ASSERT_OK(zx_handle_close(vmar));
  ASSERT_OK(zx_handle_close(process));
}

// zx_process_start() should not start second thread.
TEST(Threads, ProcessStartOnSecondThread) {
  if (getenv("NO_NEW_PROCESS")) {
    ZXTEST_SKIP("Running without the ZX_POL_NEW_PROCESS policy, skipping test case.");
  }
  static constexpr char kProcessName[] = "test-proc-thread1";
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t thread;
  ASSERT_OK(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar));
  ASSERT_OK(zx_thread_create(process, kThreadName, sizeof(kThreadName) - 1, 0, &thread));
  ASSERT_OK(zx_thread_start(thread, 0, 1, 1, 1));
  ASSERT_NE(zx_process_start(process, thread, 0, 1, ZX_HANDLE_INVALID, 1), ZX_OK);

  ASSERT_OK(zx_handle_close(thread));
  ASSERT_OK(zx_handle_close(vmar));
  ASSERT_OK(zx_handle_close(process));
}

// Test that we don't get an assertion failure (and kernel panic) if we
// pass a zero instruction pointer when starting a thread.
TEST(Threads, ThreadStartWithZeroInstructionPointer) {
  zx::result create = captive_thread::CaptiveThread::CreateRaw(kThreadName, {.pc = 0});
  ASSERT_OK(create.status_value());
  std::unique_ptr thread = *std::move(create);

  zx::result wait = thread->WaitForException();
  ASSERT_OK(wait.status_value());
  ASSERT_TRUE(thread->InException());

  EXPECT_EQ(thread->ExceptionReport()->header.type, ZX_EXCP_FATAL_PAGE_FAULT);
  EXPECT_EQ(captive_thread::FaultAddress(*thread->ExceptionReport()), 0u);
}

TEST(Threads, NonstartedThread) {
  // Perform apis against non started threads (in the INITIAL STATE).
  zx_handle_t thread;

  ASSERT_EQ(zx_thread_create(zx_process_self(), "thread", 5, 0, &thread), ZX_OK);
  ASSERT_EQ(zx_handle_close(thread), ZX_OK);
}

TEST(Threads, InfoTaskStatsFails) {
  // Spin up a thread and let it finish.
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("InfoTaskStatsFails"));
  ASSERT_NO_FATAL_FAILURE(
      test_thread.Start(threads_test_sleep_fn, zx_deadline_after(ZX_MSEC(100))));
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  // Ensure that task_stats doesn't work on it.
  zx_info_task_stats_t info;
  EXPECT_NE(
      test_thread.thread().get_info(ZX_INFO_TASK_STATS, &info, sizeof(info), nullptr, nullptr),
      ZX_OK, "Just added thread support to info_task_status?");
  // If so, replace this with a real test; see example in process.cpp.
}

TEST(Threads, InfoThreadStatsFails) {
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("InfoThreadStatsFails"));
  ASSERT_NO_FATAL_FAILURE(
      test_thread.Start(threads_test_sleep_fn, zx_deadline_after(ZX_MSEC(100))));
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  // Ensure that thread_stats doesn't work on it.
  zx_info_task_stats_t info;
  EXPECT_EQ(
      test_thread.thread().get_info(ZX_INFO_THREAD_STATS, &info, sizeof(info), nullptr, nullptr),
      ZX_ERR_BAD_STATE, "THREAD_STATS shouldn't work after a thread exits");
}

TEST(Threads, GetLastScheduledCpu) {
  zx_handle_t event;
  ASSERT_EQ(zx_event_create(0, &event), ZX_OK);

  // Create a thread.
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("GetLastScheduledCpu"));

  // Ensure "last_cpu" is ZX_INFO_INVALID_CPU prior to the thread starting.
  zx_info_thread_stats_t info;
  ASSERT_OK(
      test_thread.thread().get_info(ZX_INFO_THREAD_STATS, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(info.last_scheduled_cpu, ZX_INFO_INVALID_CPU);

  // Start the thread.
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_run_fn, event));

  // Wait for worker to start.
  ASSERT_EQ(zx_object_wait_one(event, ZX_USER_SIGNAL_0, ZX_TIME_INFINITE, /*pending=*/nullptr),
            ZX_OK);

  // Ensure the last-reported thread looks reasonable.
  ASSERT_OK(
      test_thread.thread().get_info(ZX_INFO_THREAD_STATS, &info, sizeof(info), nullptr, nullptr));
  ASSERT_NE(info.last_scheduled_cpu, ZX_INFO_INVALID_CPU);
  ASSERT_LT(info.last_scheduled_cpu, ZX_CPU_SET_MAX_CPUS);

  // Shut down and clean up.
  ASSERT_EQ(zx_object_signal(event, 0, ZX_USER_SIGNAL_1), ZX_OK);

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

template <zx_object_info_topic_t Topic, typename InfoT>
void TestThreadsGetInfoRuntime(std::string_view name) {
  zx::event event;
  ASSERT_EQ(zx::event::create(0, &event), ZX_OK);

  // Create a thread.
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init(name));

  // Ensure runtime is 0 prior to thread starting.
  InfoT info;
  size_t actual = 0;
  size_t avail = 0;
  ASSERT_EQ(test_thread.thread().get_info(Topic, &info, sizeof(info), &actual, &avail), ZX_OK);
  ASSERT_EQ(info.cpu_time, 0);
  ASSERT_EQ(info.queue_time, 0);
  EXPECT_EQ(actual, 1);
  EXPECT_EQ(avail, 1);

  // Start the thread.
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_run_fn, event.get()));

  // Wait for worker to start.
  ASSERT_EQ(event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), /*pending=*/nullptr), ZX_OK);

  // Ensure the last-reported thread looks reasonable.
  ASSERT_OK(test_thread.thread().get_info(Topic, &info, sizeof(info), nullptr, nullptr));
  ASSERT_GT(info.cpu_time, 0);
  ASSERT_GT(info.queue_time, 0);

  // Shut down and clean up.
  ASSERT_EQ(event.signal(0, ZX_USER_SIGNAL_1), ZX_OK);

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  // Ensure the runtime can still be read after the task exits.
  ASSERT_OK(test_thread.thread().get_info(Topic, &info, sizeof(info), nullptr, nullptr));
  ASSERT_GT(info.cpu_time, 0);
  ASSERT_GT(info.queue_time, 0);

  // Test that removing ZX_RIGHT_INSPECT causes runtime calls to fail.
  zx_info_handle_basic_t basic;
  ASSERT_OK(
      test_thread.thread().get_info(ZX_INFO_HANDLE_BASIC, &basic, sizeof(basic), nullptr, nullptr));
  zx::thread thread_dup;
  ASSERT_OK(test_thread.thread().duplicate(basic.rights & ~ZX_RIGHT_INSPECT, &thread_dup));
  ASSERT_EQ(thread_dup.get_info(Topic, &info, sizeof(info), nullptr, nullptr),
            ZX_ERR_ACCESS_DENIED);
}

TEST(Threads, GetInfoRuntime) {
  TestThreadsGetInfoRuntime<ZX_INFO_TASK_RUNTIME, zx_info_task_runtime_t>("GetInfoRuntime");
}

TEST(Threads, GetInfoRuntimeV1) {
  TestThreadsGetInfoRuntime<ZX_INFO_TASK_RUNTIME_V1, zx_info_task_runtime_v1_t>("GetInfoRuntimeV1");
}

TEST(Threads, GetAffinity) {
  // Create a thread.
  constexpr std::string_view kName = "GetAffinity";
  zx::thread thread;
  ASSERT_OK(zx::thread::create(*zx::process::self(), kName.data(),
                               static_cast<uint32_t>(kName.size()), 0, &thread));

  // Fetch affinity mask.
  zx_info_thread_t info;
  ASSERT_OK(thread.get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr));

  // We expect that a new thread should be runnable on at least 1 CPU.
  int num_cpus = 0;
  for (int i = 0; i < ZX_CPU_SET_MAX_CPUS; i++) {
    if (CpuMaskBitSet(info.cpu_affinity_mask, i)) {
      num_cpus++;
    }
  }
  ASSERT_GT(num_cpus, 0);

  // In the current system, we expect that a new thread will be runnable
  // on a contiguous range of CPUs, from 0 to (N - 1).
  for (int i = 0; i < ZX_CPU_SET_MAX_CPUS; i++) {
    EXPECT_EQ(CpuMaskBitSet(info.cpu_affinity_mask, i), i < num_cpus);
  }
}

TEST(Threads, ResumeSuspended) {
  zx::event event;
  ASSERT_EQ(zx::event::create(0, &event), ZX_OK);

  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("ResumeSuspended"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_wait_fn, event.get()));

  // threads_test_wait_fn() uses zx_object_wait_one() so we watch for that.
  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  // Suspend and immediately resume.
  {
    zx::suspend_token suspend_token;
    ASSERT_OK(test_thread.thread().suspend(&suspend_token));
  }

  // The thread should still be blocked on the event when it wakes up.
  // It needs to run for a bit to transition from suspended back to blocked
  // so we need to wait for it.
  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  // Check that signaling the event while suspended results in the expected behavior.
  zx::suspend_token suspend_token;
  suspend_thread_synchronous(test_thread.thread().get(), suspend_token.reset_and_get_address());

  // Verify thread is suspended.
  zx_info_thread_t info;
  ASSERT_TRUE(get_thread_info(test_thread.thread().get(), &info));
  ASSERT_EQ(info.state, ZX_THREAD_STATE_SUSPENDED);
  ASSERT_EQ(info.wait_exception_channel_type, ZX_EXCEPTION_CHANNEL_TYPE_NONE);

  // Resuming the thread should mark the thread as blocked again.
  resume_thread_synchronous(test_thread.thread().get(), suspend_token.release());

  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  // When the thread is suspended the signaling should not take effect.
  suspend_thread_synchronous(test_thread.thread().get(), suspend_token.reset_and_get_address());
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
  ASSERT_EQ(event.wait_one(ZX_USER_SIGNAL_1, zx::deadline_after(zx::msec(100)), nullptr),
            ZX_ERR_TIMED_OUT);

  suspend_token.reset();

  ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_1, zx::time::infinite(), nullptr));

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

TEST(Threads, SuspendSleeping) {
  const zx_instant_mono_t sleep_deadline = zx_deadline_after(ZX_MSEC(100));

  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SuspendSleeping"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_sleep_fn, sleep_deadline));

  // We can't simply wait until the thread is blocked sleeping because it may
  // complete its sleep and terminate before we get a chance to observe it.
  constexpr std::array kStates = {
      ZX_THREAD_STATE_BLOCKED_SLEEPING,
      ZX_THREAD_STATE_DYING,
      ZX_THREAD_STATE_DEAD,
  };
  wait_thread_state(test_thread.thread().get(), std::span(kStates));

  // Suspend the thread.
  zx::suspend_token suspend_token;
  zx_status_t status = test_thread.thread().suspend(&suspend_token);
  if (status != ZX_OK) {
    ASSERT_EQ(status, ZX_ERR_BAD_STATE);
    // This might happen if the thread exits before we tried suspending it
    // (due to e.g. a long context-switch away).  The system is too loaded
    // and so we might not have a chance at success here without a massive
    // sleep duration.
    zx_info_thread_t info;
    ASSERT_OK(test_thread.thread().get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr));
    ASSERT_TRUE(info.state == ZX_THREAD_STATE_DEAD || info.state == ZX_THREAD_STATE_DYING,
                "info.state=%d\n", info.state);
    // Early bail from the test, since we hit a possible race from an
    // overloaded machine.
    return;
  }
  ASSERT_OK(test_thread.thread().wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), nullptr));
  suspend_token.reset();

  // Wait for the sleep to finish.
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  const zx_instant_mono_t now = zx_clock_get_monotonic();
  ASSERT_GE(now, sleep_deadline, "thread did not sleep long enough");
}

TEST(Threads, SuspendChannelCall) {
  zx_handle_t channel;
  channel_call_suspend_test_arg thread_arg;
  ASSERT_EQ(zx_channel_create(0, &thread_arg.channel, &channel), ZX_OK);
  thread_arg.call_status = ZX_ERR_BAD_STATE;

  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SuspendChannelCall"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_channel_call_fn, &thread_arg));

  // Wait for the thread to send a channel call before suspending it
  ASSERT_EQ(zx_object_wait_one(channel, ZX_CHANNEL_READABLE, ZX_TIME_INFINITE, NULL), ZX_OK);

  // Suspend the thread.
  zx::suspend_token suspend_token;
  suspend_thread_synchronous(test_thread.thread().get(), suspend_token.reset_and_get_address());

  // Read the message
  uint8_t buf[9];
  uint32_t actual_bytes;
  ASSERT_EQ(zx_channel_read(channel, 0, buf, NULL, sizeof(buf), 0, &actual_bytes, NULL), ZX_OK);
  ASSERT_EQ(actual_bytes, sizeof(buf));
  ASSERT_EQ(memcmp(buf + sizeof(zx_txid_t), &"abcdefghi"[sizeof(zx_txid_t)],
                   sizeof(buf) - sizeof(zx_txid_t)),
            0);

  // Write a reply
  buf[8] = 'j';
  ASSERT_EQ(zx_channel_write(channel, 0, buf, sizeof(buf), NULL, 0), ZX_OK);

  // Make sure the remote channel didn't get signaled
  EXPECT_EQ(zx_object_wait_one(thread_arg.channel, ZX_CHANNEL_READABLE, 0, NULL), ZX_ERR_TIMED_OUT);

  // Make sure we can't read from the remote channel (the message should have
  // been reserved for the other thread, even though it is suspended).
  EXPECT_EQ(zx_channel_read(thread_arg.channel, 0, buf, NULL, sizeof(buf), 0, &actual_bytes, NULL),
            ZX_ERR_SHOULD_WAIT);

  // Wake the suspended thread
  suspend_token.reset();

  // Wait for the thread to finish
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  EXPECT_EQ(thread_arg.call_status, ZX_OK);

  ASSERT_EQ(zx_handle_close(channel), ZX_OK);
}

TEST(Threads, SuspendPortCall) {
  zx_handle_t port[2];
  ASSERT_EQ(zx_port_create(0, &port[0]), ZX_OK);
  ASSERT_EQ(zx_port_create(0, &port[1]), ZX_OK);

  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SuspendPortCall"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_port_fn, port));

  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_PORT);

  zx::suspend_token suspend_token;
  ASSERT_OK(test_thread.thread().suspend(&suspend_token));

  zx_port_packet_t packet1 = {100ull, ZX_PKT_TYPE_USER, 0u, {}};
  zx_port_packet_t packet2 = {300ull, ZX_PKT_TYPE_USER, 0u, {}};

  ASSERT_EQ(zx_port_queue(port[0], &packet1), ZX_OK);
  ASSERT_EQ(zx_port_queue(port[0], &packet2), ZX_OK);

  zx_port_packet_t packet;
  ASSERT_EQ(zx_port_wait(port[1], zx_deadline_after(ZX_MSEC(100)), &packet), ZX_ERR_TIMED_OUT);

  suspend_token.reset();

  ASSERT_EQ(zx_port_wait(port[1], ZX_TIME_INFINITE, &packet), ZX_OK);
  EXPECT_EQ(packet.key, 105ull);

  ASSERT_EQ(zx_port_wait(port[0], ZX_TIME_INFINITE, &packet), ZX_OK);
  EXPECT_EQ(packet.key, 300ull);

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  ASSERT_EQ(zx_handle_close(port[0]), ZX_OK);
  ASSERT_EQ(zx_handle_close(port[1]), ZX_OK);
}

TEST(Threads, SuspendStopsThread) {
  std::atomic_int value = kTestAtomicClobberValue;
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SuspendStopsThread"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_atomic_store, &value));

  while (value.load() != kTestAtomicSetValue) {
    zx_thread_legacy_yield(0ull);
  }

  // Suspend the thread and wait for the suspend to happen.

  zx::suspend_token suspend_token;
  ASSERT_OK(test_thread.thread().suspend(&suspend_token));
  ASSERT_OK(test_thread.thread().wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), nullptr));

  // Clobber the value, wait, and "check" that the thread didn't reset the value.
  // This isn't fool proof, but it's hard to check that something "doesn't" happen.
  value.store(kTestAtomicClobberValue);
  zx_nanosleep(zx_deadline_after(ZX_MSEC(50)));
  ASSERT_EQ(atomic_load(&value), kTestAtomicClobberValue);

  // Let the thread resume and then wait for it to reset the value.
  suspend_token.reset();
  while (value.load() != kTestAtomicSetValue) {
    zx_thread_legacy_yield(0ull);
  }

  // Clean up.
  value.store(kTestAtomicExitValue);

  // Wait for the thread termination to complete.  We should do this so
  // that any later tests which handle process debug exceptions do not
  // receive an ZX_EXCP_THREAD_EXITING event.

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

TEST(Threads, SuspendMultiple) {
  zx::event event;
  ASSERT_EQ(zx::event::create(0, &event), ZX_OK);

  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SuspendMultiple"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_wait_break_fn, event.get()));

  // The thread will now be blocked on the event. Wake it up and catch the trap (undefined
  // exception).
  zx_handle_t exception_channel, exception;
  ASSERT_EQ(zx_task_create_exception_channel(zx_process_self(), ZX_EXCEPTION_CHANNEL_DEBUGGER,
                                             &exception_channel),
            ZX_OK);
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
  wait_thread_excp_type(test_thread.thread().get(), exception_channel, ZX_EXCP_SW_BREAKPOINT,
                        ZX_EXCP_THREAD_STARTING, &exception);

  // The thread should now be blocked on a debugger exception.
  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_EXCEPTION);
  zx_info_thread_t info;
  ASSERT_TRUE(get_thread_info(test_thread.thread().get(), &info));
  ASSERT_EQ(info.wait_exception_channel_type, ZX_EXCEPTION_CHANNEL_TYPE_DEBUGGER);

  advance_over_breakpoint(test_thread.thread().get());

  // Suspend twice (on top of the existing exception). Don't use the synchronous suspend since
  // suspends don't escape out of exception handling, unlike blocking
  // syscalls where suspend will escape out of them.
  zx::suspend_token suspend_token1, suspend_token2;
  ASSERT_OK(test_thread.thread().suspend(&suspend_token1));
  ASSERT_OK(test_thread.thread().suspend(&suspend_token2));

  // Resume one token, it should remain blocked.
  suspend_token1.reset();
  ASSERT_TRUE(get_thread_info(test_thread.thread().get(), &info));
  // Note: If this check is flaky, it's failing. It should not transition out of the blocked
  // state, but if it does so, it will do so asynchronously which might cause
  // nondeterministic failures.
  ASSERT_EQ(info.state, ZX_THREAD_STATE_BLOCKED_EXCEPTION);

  // Resume the exception. It should be SUSPENDED now that the exception is complete (one could
  // argue that it could still be BLOCKED also, but it's not in the current implementation).
  // The transition to SUSPENDED happens asynchronously unlike some of the exception states.
  uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
  ASSERT_EQ(zx_object_set_property(exception, ZX_PROP_EXCEPTION_STATE, &state, sizeof(state)),
            ZX_OK);
  ASSERT_EQ(zx_handle_close(exception), ZX_OK);
  zx_signals_t observed = 0u;
  ASSERT_OK(test_thread.thread().wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), &observed));

  ASSERT_TRUE(get_thread_info(test_thread.thread().get(), &info));
  ASSERT_EQ(info.state, ZX_THREAD_STATE_SUSPENDED);

  // 2nd resume, should be running and then exit.
  suspend_token2.reset();

  // We have to close the exception channel because it will keep the thread in DYING state.
  ASSERT_EQ(zx_handle_close(exception_channel), ZX_OK);

  // Clean up.
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

TEST(Threads, SuspendInvalidTargets) {
  zx_handle_t suspend_token;
  // Suspend self thread: NOT_SUPPORTED
  EXPECT_EQ(zx_task_suspend(zx_thread_self(), &suspend_token), ZX_ERR_NOT_SUPPORTED);

  // Suspend self process: NOT_SUPPORTED
  EXPECT_EQ(zx_task_suspend(zx_process_self(), &suspend_token), ZX_ERR_NOT_SUPPORTED);

  // Suspend wrong type (VMO): WRONG_TYPE
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1024, 0, &vmo));
  EXPECT_EQ(zx_task_suspend(vmo.get(), &suspend_token), ZX_ERR_WRONG_TYPE);
}

TEST(Threads, SuspendAfterDeath) {
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SuspendAfterDeath"));
  ASSERT_NO_FATAL_FAILURE(
      test_thread.Start(threads_test_sleep_fn, zx_deadline_after(ZX_MSEC(100))));
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  zx::suspend_token suspend_token;
  EXPECT_EQ(test_thread.thread().suspend(&suspend_token), ZX_ERR_BAD_STATE);
  EXPECT_FALSE(suspend_token.is_valid());
}

// Suspend a thread before starting and make sure it starts into suspended state.
TEST(Threads, StartSuspendedThread) {
  TestThread test_thread;

  // Make a test thread with a bespoke stack VMO that starts out empty.
  // Its stack will be mapped, but those mappings will still fault if used.
  zx::vmo stack_vmo;
  ASSERT_OK(zx::vmo::create(0, ZX_VMO_RESIZABLE, &stack_vmo));

  ASSERT_NO_FATAL_FAILURE(test_thread.Init("StartSuspendedThread", stack_vmo.borrow()));

  // Suspend first, then start the thread.
  zx::suspend_token suspend_token;
  ASSERT_OK(test_thread.thread().suspend(&suspend_token));

  std::atomic_int value = 0;
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_atomic_store, &value));

  // Make sure the thread goes directly to suspended state without executing at all.
  ASSERT_OK(test_thread.thread().wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), nullptr));

  // Once we know it's suspended, give it a real stack.
  ASSERT_OK(stack_vmo.set_size(TestThread::kDefaultStackSize));

  // Make sure the thread still resumes properly.
  suspend_token.reset();
  ASSERT_OK(test_thread.thread().wait_one(ZX_THREAD_RUNNING, zx::time::infinite(), nullptr));
  while (value != kTestAtomicSetValue) {
    zx_thread_legacy_yield(0ull);
  }

  // Clean up.
  value.store(kTestAtomicExitValue);

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

// Suspend and resume a thread before starting, it should start as normal.
TEST(Threads, StartSuspendedAndResumedThread) {
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("StartSuspendedAndResumedThread"));

  // Suspend and resume.
  {
    zx::suspend_token suspend_token;
    ASSERT_OK(test_thread.thread().suspend(&suspend_token));
  }

  // Start the thread, it should behave normally.
  std::atomic_int value = 0;
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_atomic_store, &value));

  ASSERT_OK(test_thread.thread().wait_one(ZX_THREAD_RUNNING, zx::time::infinite(), nullptr));
  while (value != 1) {
    zx_thread_legacy_yield(0ull);
  }

  // Clean up.
  value.store(kTestAtomicExitValue);

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

void jump_to_thread_exit(zx_handle_t thread) {
  const uint64_t exit_pc = reinterpret_cast<uintptr_t>(&zx_thread_exit);
  zx_thread_state_general_regs_t regs;
  ASSERT_EQ(zx_thread_read_state(thread, ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)), ZX_OK);
#if defined(__aarch64__) || defined(__riscv)
  regs.pc = exit_pc;
#elif defined(__x86_64__)
  regs.rip = exit_pc;
#else
#error Not supported on this platform.
#endif
  ASSERT_EQ(zx_thread_write_state(thread, ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)),
            ZX_OK);
}

void TestBadSyscall(std::string_view name, uint64_t syscall_number) {
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init(name));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  const bad_syscall_arg arg = {
      .event = event.get(),
      .syscall_number = syscall_number,
  };
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_bad_syscall_fn, &arg));

  // The thread will now be blocked on the event. Wake it up and catch the bad syscall
  // exception.
  zx_handle_t exception_channel, exception;
  ASSERT_OK(zx_task_create_exception_channel(zx_process_self(), 0, &exception_channel));
  ASSERT_OK(zx_object_signal(arg.event, 0, ZX_USER_SIGNAL_0));
  wait_thread_excp_type(test_thread.thread().get(), exception_channel, ZX_EXCP_POLICY_ERROR,
                        ZX_EXCP_THREAD_STARTING, &exception);

  zx_exception_report_t report = {};
  ASSERT_OK(test_thread.thread().get_info(ZX_INFO_THREAD_EXCEPTION_REPORT, &report, sizeof(report),
                                          nullptr, nullptr));

  ASSERT_EQ(report.header.type, ZX_EXCP_POLICY_ERROR);
  ASSERT_EQ(report.context.synth_code, ZX_EXCP_POLICY_CODE_BAD_SYSCALL);
  ASSERT_EQ(report.context.synth_data, syscall_number);

  jump_to_thread_exit(test_thread.thread().get());

  uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
  ASSERT_OK(zx_object_set_property(exception, ZX_PROP_EXCEPTION_STATE, &state, sizeof(state)));
  ASSERT_OK(zx_handle_close(exception));

  ASSERT_EQ(zx_handle_close(exception_channel), ZX_OK);

  // Clean up.
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

// Test both valid and invalid syscall numbers.  The valid syscall number
// should fail anyway because the call is not made from the vDSO.
TEST(Threads, BadSyscallCaller) { TestBadSyscall("BadSyscallCaller", 1); }
TEST(Threads, BadSyscallNumber) { TestBadSyscall("BadSyscallNumber", 10000); }

TEST(Threads, ExitViaException) {
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("ExitViaException"));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  const bad_syscall_arg arg = {.event = event.get(), .syscall_number = 1};
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_bad_syscall_fn, &arg));

  // The thread will now be blocked on the event. Wake it up and catch the bad syscall
  // exception.
  zx_handle_t exception_channel, exception;
  ASSERT_OK(zx_task_create_exception_channel(zx_process_self(), 0, &exception_channel));
  ASSERT_OK(zx_object_signal(arg.event, 0, ZX_USER_SIGNAL_0));
  wait_thread_excp_type(test_thread.thread().get(), exception_channel, ZX_EXCP_POLICY_ERROR,
                        ZX_EXCP_THREAD_STARTING, &exception);

  // Notice that we do not jump to thread exit, unlike in Thread.BadSyscall.
  // Instead, we terminate the thread using ZX_EXCEPTION_STATE_THREAD_EXIT.

  uint32_t state = ZX_EXCEPTION_STATE_THREAD_EXIT;
  ASSERT_OK(zx_object_set_property(exception, ZX_PROP_EXCEPTION_STATE, &state, sizeof(state)));
  ASSERT_OK(zx_handle_close(exception));

  ASSERT_EQ(zx_handle_close(exception_channel), ZX_OK);

  // Clean up.
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

void port_wait_for_signal(zx_handle_t port, zx_handle_t thread, zx_instant_mono_t deadline,
                          zx_signals_t mask, zx_port_packet_t* packet) {
  ASSERT_EQ(zx_object_wait_async(thread, port, 0u, mask, 0), ZX_OK);
  ASSERT_EQ(zx_port_wait(port, deadline, packet), ZX_OK);
  ASSERT_EQ(packet->type, ZX_PKT_TYPE_SIGNAL_ONE);
}

// Test signal delivery of suspended threads via single async wait.
TEST(Threads, SuspendSingleWaitAsyncSignalDelivery) {
  zx_handle_t event;
  ASSERT_EQ(zx_event_create(0, &event), ZX_OK);

  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SuspendSingleWaitAsyncSignalDelivery"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_wait_fn, event));

  zx_handle_t port;
  ASSERT_EQ(zx_port_create(0, &port), ZX_OK);

  // There should be a RUNNING signal packet present and not SUSPENDED.
  // This is from when the thread first started to run.
  constexpr zx_signals_t kRunSuspMask = ZX_THREAD_RUNNING | ZX_THREAD_SUSPENDED;
  zx_port_packet_t packet;
  ASSERT_NO_FATAL_FAILURE(
      port_wait_for_signal(port, test_thread.thread().get(), 0u, kRunSuspMask, &packet));
  ASSERT_EQ(packet.signal.observed & kRunSuspMask, ZX_THREAD_RUNNING);

  // Make sure there are no more packets.
  // RUNNING or SUSPENDED is always asserted.
  ASSERT_OK(test_thread.thread().wait_async(*zx::unowned_port{port}, 0u, ZX_THREAD_SUSPENDED, 0));
  ASSERT_EQ(zx_port_wait(port, 0u, &packet), ZX_ERR_TIMED_OUT);
  ASSERT_EQ(zx_port_cancel(port, test_thread.thread().get(), 0u), ZX_OK);

  zx_handle_t suspend_token = ZX_HANDLE_INVALID;
  suspend_thread_synchronous(test_thread.thread().get(), &suspend_token);

  zx_info_thread_t info;
  ASSERT_TRUE(get_thread_info(test_thread.thread().get(), &info));
  ASSERT_EQ(info.state, ZX_THREAD_STATE_SUSPENDED);

  resume_thread_synchronous(test_thread.thread().get(), suspend_token);
  ASSERT_TRUE(get_thread_info(test_thread.thread().get(), &info));
  // At this point the thread may be running or blocked waiting for an
  // event. Either one is fine. threads_test_wait_fn() uses
  // zx_object_wait_one() so we watch for that.
  ASSERT_TRUE(info.state == ZX_THREAD_STATE_RUNNING ||
              info.state == ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  // We should see just RUNNING,
  // and it should be immediately present (no deadline).
  port_wait_for_signal(port, test_thread.thread().get(), 0u, kRunSuspMask, &packet);
  ASSERT_EQ(packet.signal.observed & kRunSuspMask, ZX_THREAD_RUNNING);

  // The thread should still be blocked on the event when it wakes up.
  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  // Check that suspend/resume while blocked in a syscall results in
  // the expected behavior and is visible via async wait.
  suspend_token = ZX_HANDLE_INVALID;
  ASSERT_EQ(zx_task_suspend(test_thread.thread().get(), &suspend_token), ZX_OK);
  port_wait_for_signal(port, test_thread.thread().get(), zx_deadline_after(ZX_SEC(180)),
                       ZX_THREAD_SUSPENDED, &packet);
  ASSERT_EQ(packet.signal.observed & kRunSuspMask, ZX_THREAD_SUSPENDED);

  ASSERT_TRUE(get_thread_info(test_thread.thread().get(), &info));
  ASSERT_EQ(info.state, ZX_THREAD_STATE_SUSPENDED);
  ASSERT_EQ(zx_handle_close(suspend_token), ZX_OK);
  port_wait_for_signal(port, test_thread.thread().get(), zx_deadline_after(ZX_SEC(180)),
                       ZX_THREAD_RUNNING, &packet);
  ASSERT_EQ(packet.signal.observed & kRunSuspMask, ZX_THREAD_RUNNING);

  // Resumption from being suspended back into a blocking syscall will be
  // in the RUNNING state and then BLOCKED.
  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  ASSERT_EQ(zx_object_signal(event, 0, ZX_USER_SIGNAL_0), ZX_OK);
  ASSERT_EQ(zx_object_wait_one(event, ZX_USER_SIGNAL_1, ZX_TIME_INFINITE, NULL), ZX_OK);

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  ASSERT_EQ(zx_handle_close(port), ZX_OK);
  ASSERT_EQ(zx_handle_close(event), ZX_OK);
}

// Helper class for setting up a test for reading register state from a worker thread.
template <typename RegisterStruct>
class RegisterReadSetup {
 public:
  using ThreadFunc = void (*)(RegisterStruct*);

  RegisterReadSetup() = default;
  ~RegisterReadSetup() { ExitThread(); }

  zx_handle_t thread_handle() const { return test_thread_.thread().get(); }

  // Run |thread_func| with |state|.  Once the thread reaches |expected_pc|, return, leaving the
  // thread suspended.
  void RunUntil(std::string_view name, ThreadFunc thread_func, RegisterStruct* state,
                uintptr_t expected_pc) {
    ASSERT_NO_FATAL_FAILURE(test_thread_.Init(name));
    ASSERT_NO_FATAL_FAILURE(test_thread_.Start(thread_func, state));
    while (true) {
      ASSERT_EQ(zx_nanosleep(zx_deadline_after(ZX_MSEC(1))), ZX_OK);
      ASSERT_NO_FATAL_FAILURE(Suspend());
      zx_thread_state_general_regs_t regs;
      ASSERT_EQ(
          zx_thread_read_state(thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)),
          ZX_OK);
      if (regs.REG_PC == expected_pc) {
        break;
      }
      ASSERT_NO_FATAL_FAILURE(Resume());
    }
  }

  void Resume() { resume_thread_synchronous(thread_handle(), suspend_token_.release()); }

  void Suspend() {
    suspend_thread_synchronous(thread_handle(), suspend_token_.reset_and_get_address());
  }

  void ExitThread() {
    zx_info_thread_t info;
    ASSERT_TRUE(get_thread_info(thread_handle(), &info));
    if (info.state != ZX_THREAD_STATE_SUSPENDED) {
      Suspend();
    }

    zx_thread_state_general_regs_t regs;
    ASSERT_EQ(
        zx_thread_read_state(thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)),
        ZX_OK);

#if defined(__aarch64__) || defined(__riscv)
    regs.pc = reinterpret_cast<uintptr_t>(zx_thread_exit);
#elif defined(__x86_64__)
    regs.rip = reinterpret_cast<uintptr_t>(zx_thread_exit);
#else
#error "what machine?"
#endif

    ASSERT_EQ(
        zx_thread_write_state(thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)),
        ZX_OK);

    suspend_token_.reset();

    // Wait for the thread termination to complete.
    ASSERT_NO_FATAL_FAILURE(test_thread_.Wait());
  }

 private:
  TestThread test_thread_;
  zx::suspend_token suspend_token_;
};

// This tests the registers reported by zx_thread_read_state() for a
// suspended thread.  It starts a thread which sets all the registers to
// known test values.
TEST(Threads, ReadingGeneralRegisterState) {
  zx_thread_state_general_regs_t gen_regs_expected;
  general_regs_fill_test_values(&gen_regs_expected);
  gen_regs_expected.REG_PC = (uintptr_t)spin_address;

  RegisterReadSetup<zx_thread_state_general_regs_t> setup;
  setup.RunUntil("ReadingGeneralRegisterState", &spin_with_general_regs, &gen_regs_expected,
                 reinterpret_cast<uintptr_t>(&spin_address));

  zx_thread_state_general_regs_t regs;
  ASSERT_EQ(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &regs,
                                 sizeof(regs)),
            ZX_OK);
  ASSERT_NO_FATAL_FAILURE(general_regs_expect_eq(regs, gen_regs_expected));
}

TEST(Threads, ReadingFpRegisterState) {
  zx_thread_state_fp_regs_t fp_regs_expected;
  fp_regs_fill_test_values(&fp_regs_expected);

  RegisterReadSetup<zx_thread_state_fp_regs_t> setup;
  setup.RunUntil("ReadingFpRegisterState", &spin_with_fp_regs, &fp_regs_expected,
                 reinterpret_cast<uintptr_t>(&spin_address));

  zx_thread_state_fp_regs_t regs;
  zx_status_t status =
      zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_FP_REGS, &regs, sizeof(regs));
#if defined(__x86_64__) || defined(__riscv)
  ASSERT_EQ(status, ZX_OK);
  ASSERT_NO_FATAL_FAILURE(fp_regs_expect_eq(regs, fp_regs_expected));
#elif defined(__aarch64__)
  ASSERT_EQ(status, ZX_ERR_NOT_SUPPORTED);
#else
#error unsupported platform
#endif
}

TEST(Threads, ReadingVectorRegisterState) {
  zx_thread_state_vector_regs_t vector_regs_expected;
  vector_regs_fill_test_values(&vector_regs_expected);

  RegisterReadSetup<zx_thread_state_vector_regs_t> setup;
  setup.RunUntil("ReadingVectorRegisterState", &spin_with_vector_regs, &vector_regs_expected,
                 reinterpret_cast<uintptr_t>(&spin_address));

  zx_thread_state_vector_regs_t regs;
  memset(&regs, 0xff, sizeof(regs));
  ASSERT_EQ(
      zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_VECTOR_REGS, &regs, sizeof(regs)),
      ZX_OK);

  ASSERT_NO_FATAL_FAILURE(vector_regs_expect_unsupported_are_zero(regs));
  ASSERT_NO_FATAL_FAILURE(vector_regs_expect_eq(regs, vector_regs_expected));
}

// Procedure:
//  1. Call Init() which will start a thread and suspend it.
//  2. Write the register state you want to the thread_handle().
//  3. Call DoSave with the save function and pointer. This will execute that code in the context of
//     the thread.
template <typename RegisterStruct>
class RegisterWriteSetup {
 public:
  using SaveFunc = void (*)();

  RegisterWriteSetup() = default;

  ~RegisterWriteSetup() {
    // If the thread is still suspended, it means we have not called DoSave
    // yet and the thread is still hanging. We need to terminate it.
    if (suspend_token_ != ZX_HANDLE_INVALID) {
      ASSERT_NO_FATAL_FAILURE(ExitThread());
    }
  }

  zx_handle_t thread_handle() const { return test_thread_.thread().get(); }

  void Init(std::string_view name) {
    ASSERT_NO_FATAL_FAILURE(test_thread_.Init(name));
    ASSERT_NO_FATAL_FAILURE(test_thread_.Start(threads_test_atomic_store, &p_));

    // Wait for the thread to begin executing.
    while (p_.load() == 0) {
      zx_nanosleep(zx_deadline_after(THREAD_BLOCKED_WAIT_DURATION));
    }

    suspend_thread_synchronous(thread_handle(), suspend_token_.reset_and_get_address());
  }

  // The IP and SP set in the general registers will be filled in to the optional output
  // parameters. This is for the general register test since we change those values out from
  // under it.
  void DoSave(SaveFunc save_func, RegisterStruct* out, uint64_t* general_ip = nullptr,
              uint64_t* general_sp = nullptr) {
    // Modify the PC to point to the routine, and the SP to point to the output struct.
    zx_thread_state_general_regs_t general_regs;
    ASSERT_EQ(zx_thread_read_state(thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &general_regs,
                                   sizeof(general_regs)),
              ZX_OK);

    struct {
      // A small stack that is used for calling zx_thread_exit().
      char stack[1024] __ALIGNED(16);
      RegisterStruct regs_got;  // STACK_PTR will point here.
    } stack;
    general_regs.REG_PC = (uintptr_t)save_func;
    general_regs.REG_STACK_PTR = (uintptr_t)(stack.stack + sizeof(stack.stack));
    ASSERT_EQ(zx_thread_write_state(thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &general_regs,
                                    sizeof(general_regs)),
              ZX_OK);

    if (general_ip)
      *general_ip = general_regs.REG_PC;
    if (general_sp)
      *general_sp = general_regs.REG_STACK_PTR;

    // Unsuspend the thread and wait for it to finish executing, this will run the code
    // and fill the RegisterStruct we passed.
    suspend_token_.reset();
    ASSERT_NO_FATAL_FAILURE(test_thread_.Wait());

    memcpy(out, &stack.regs_got, sizeof(RegisterStruct));
  }

  void ExitThread() {
    ASSERT_TRUE(suspend_token_.is_valid());

    // Tell the thread to exit.
    p_.store(kTestAtomicExitValue);

    // Resume the thread.
    suspend_token_.reset();

    // Wait for the thread termination to complete.
    ASSERT_NO_FATAL_FAILURE(test_thread_.Wait());

    test_thread_ = {};
  }

 private:
  std::atomic_int p_ = 0;
  TestThread test_thread_;
  zx::suspend_token suspend_token_;
};

// This tests writing registers using zx_thread_write_state().  After
// setting registers using that syscall, it reads back the registers and
// checks their values.
TEST(Threads, WritingGeneralRegisterState) {
  RegisterWriteSetup<zx_thread_state_general_regs_t> setup;
  setup.Init("WritingGeneralRegisterState");

  // Set the general registers.
  zx_thread_state_general_regs_t regs_to_set;
  general_regs_fill_test_values(&regs_to_set);
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &regs_to_set,
                                  sizeof(regs_to_set)),
            ZX_OK);

  zx_thread_state_general_regs_t regs;
  uint64_t ip = 0, sp = 0;
  setup.DoSave(&save_general_regs_and_exit_thread, &regs, &ip, &sp);

  // Fix up the expected values with the IP/SP required for the register read.
  regs_to_set.REG_PC = ip;
  regs_to_set.REG_STACK_PTR = sp;
  ASSERT_NO_FATAL_FAILURE(general_regs_expect_eq(regs_to_set, regs));
}

// This tests writing single step state using zx_thread_write_state().
TEST(Threads, WritingSingleStepState) {
  RegisterWriteSetup<zx_thread_state_single_step_t> setup;
  setup.Init("WritingSingleStepState");

#if !defined(__riscv)
  // 0 is valid.
  zx_thread_state_single_step_t single_step = 0;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_SINGLE_STEP, &single_step,
                                  sizeof(single_step)),
            ZX_OK);

  // 1 is valid.
  single_step = 1;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_SINGLE_STEP, &single_step,
                                  sizeof(single_step)),
            ZX_OK);

  // All other values are invalid.
  single_step = 2;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_SINGLE_STEP, &single_step,
                                  sizeof(single_step)),
            ZX_ERR_INVALID_ARGS);
  single_step = UINT32_MAX;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_SINGLE_STEP, &single_step,
                                  sizeof(single_step)),
            ZX_ERR_INVALID_ARGS);

  // Buffer can be larger than necessary.
  single_step = 0;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_SINGLE_STEP, &single_step,
                                  sizeof(single_step) + 1),
            ZX_OK);
  // But not smaller.
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_SINGLE_STEP, &single_step,
                                  sizeof(single_step) - 1),
            ZX_ERR_BUFFER_TOO_SMALL);
#else
  // RISC-V currently does not allow setting of the single step state.
  zx_thread_state_single_step_t single_step = 0;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_SINGLE_STEP, &single_step,
                                  sizeof(single_step)),
            ZX_ERR_NOT_SUPPORTED);
#endif
}

TEST(Threads, WritingFpRegisterState) {
  RegisterWriteSetup<zx_thread_state_fp_regs_t> setup;
  setup.Init("WritingFpRegisterState");

  // The busyloop code executed initially by the setup class will have executed an MMX instruction
  // so that the MMX state is available to write.
  zx_thread_state_fp_regs_t regs_to_set;
  fp_regs_fill_test_values(&regs_to_set);

  zx_status_t status = zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_FP_REGS,
                                             &regs_to_set, sizeof(regs_to_set));

#if defined(__x86_64__) || defined(__riscv)
  ASSERT_EQ(status, ZX_OK);

  zx_thread_state_fp_regs_t regs;
  setup.DoSave(&save_fp_regs_and_exit_thread, &regs);
  ASSERT_NO_FATAL_FAILURE(fp_regs_expect_eq(regs_to_set, regs));
#elif defined(__aarch64__)
  ASSERT_EQ(status, ZX_ERR_NOT_SUPPORTED);
#else
#error unsupported platform
#endif
}

TEST(Threads, WritingVectorRegisterState) {
  RegisterWriteSetup<zx_thread_state_vector_regs_t> setup;
  setup.Init("WritingVectorRegisterState");

  zx_thread_state_vector_regs_t regs_to_set;
  vector_regs_fill_test_values(&regs_to_set);
  ASSERT_NO_FATAL_FAILURE(vector_regs_expect_unsupported_are_zero(regs_to_set));

  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_VECTOR_REGS, &regs_to_set,
                                  sizeof(regs_to_set)),
            ZX_OK);

  zx_thread_state_vector_regs_t regs;
  setup.DoSave(&save_vector_regs_and_exit_thread, &regs);
  ASSERT_NO_FATAL_FAILURE(vector_regs_expect_eq(regs_to_set, regs));
}

TEST(Threads, WritingVectorRegisterStateUnsupportedFieldsIgnored) {
  RegisterWriteSetup<zx_thread_state_vector_regs_t> setup;
  setup.Init("WritingVectorRegisterStateUnsupportedFieldsIgnored");

  zx_thread_state_vector_regs_t regs;
  vector_regs_fill_test_values(&regs);

#if defined(__x86_64__)
  // Fill in the fields corresponding to unsupported features so we can later verify they are zeroed
  // out by |zx_thread_read_state|.
  for (int reg = 0; reg < 16; reg++) {
    for (int i = 5; i < 8; i++) {
      regs.zmm[reg].v[i] = 0xfffffffffffffffful;
    }
  }
  for (int reg = 16; reg < 32; reg++) {
    for (int i = 0; i < 8; i++) {
      regs.zmm[reg].v[i] = 0xfffffffffffffffful;
    }
  }
#endif

  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_VECTOR_REGS, &regs,
                                  sizeof(regs)),
            ZX_OK);
  ASSERT_EQ(
      zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_VECTOR_REGS, &regs, sizeof(regs)),
      ZX_OK);

  ASSERT_NO_FATAL_FAILURE(vector_regs_expect_unsupported_are_zero(regs));

  zx_thread_state_vector_regs_t vector_regs_expected;
  vector_regs_fill_test_values(&vector_regs_expected);
  ASSERT_NO_FATAL_FAILURE(vector_regs_expect_eq(regs, vector_regs_expected));
}

// Test for https://fxbug.dev/42127734: Make sure zx_thread_write_state doesn't overwrite
// reserved bits in mxcsr (x64 only).
TEST(Threads, WriteThreadStateWithInvalidMxcsrIsInvalidArgs) {
#if defined(__x86_64__)
  RegisterWriteSetup<zx_thread_state_vector_regs_t> setup;
  setup.Init("WriteThreadStateWithInvalidMxcsrIsInvalidArgs");

  zx_thread_state_vector_regs_t start_values;
  ASSERT_OK(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_VECTOR_REGS, &start_values,
                                 sizeof(start_values)));

  zx_thread_state_vector_regs_t regs_to_set;
  vector_regs_fill_test_values(&regs_to_set);
  regs_to_set.mxcsr = 0xffffffff;

  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_VECTOR_REGS, &regs_to_set,
                                  sizeof(regs_to_set)));

  zx_thread_state_vector_regs_t end_values;
  setup.DoSave(&save_vector_regs_and_exit_thread, &end_values);
  ASSERT_NO_FATAL_FAILURE(vector_regs_expect_eq(start_values, end_values));
#endif  // defined(__x86_64__)
}

// This test starts a thread which reads and writes from TLS.
TEST(Threads, ThreadLocalRegisterState) {
  RegisterWriteSetup<struct thread_local_regs> setup;
  setup.Init("ThreadLocalRegisterState");

  zx_thread_state_general_regs_t regs = {};

#if defined(__x86_64__)
  // The thread will read these from the fs and gs base addresses
  // into the output regs struct, and then write different numbers.
  uint64_t fs_base_value = 0x1234;
  uint64_t gs_base_value = 0x5678;
  regs.fs_base = (uintptr_t)&fs_base_value;
  regs.gs_base = (uintptr_t)&gs_base_value;
#elif defined(__aarch64__)
  uint64_t tpidr_value = 0x1234;
  regs.tpidr = (uintptr_t)&tpidr_value;
#elif defined(__riscv)
  uint64_t tp_value = 0x1234;
  regs.tp = (uintptr_t)&tp_value;
#else
#error "what machine?"
#endif

  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &regs,
                                  sizeof(regs)),
            ZX_OK);

  struct thread_local_regs tls_regs;
  setup.DoSave(&save_thread_local_regs_and_exit_thread, &tls_regs);

#if defined(__x86_64__)
  EXPECT_EQ(tls_regs.fs_base_value, 0x1234);
  EXPECT_EQ(tls_regs.gs_base_value, 0x5678);
  EXPECT_EQ(fs_base_value, 0x12345678);
  EXPECT_EQ(gs_base_value, 0x7890abcd);
#elif defined(__aarch64__)
  EXPECT_EQ(tls_regs.tpidr_value, 0x1234);
  EXPECT_EQ(tpidr_value, 0x12345678);
#elif defined(__riscv)
  EXPECT_EQ(tls_regs.tp_value, 0x1234);
  EXPECT_EQ(tp_value, 0x12345678);
#else
#error "what machine?"
#endif
}

#if defined(__x86_64__)

#include <cpuid.h>

// This is based on code from kernel/ which isn't usable by code in system/.
enum { X86_CPUID_HYP_VENDOR = 0x40000000 };

bool x86_is_kvm_hypervisor() {
  uint32_t regs[4];
  __cpuid(X86_CPUID_HYP_VENDOR, regs[0], regs[1], regs[2], regs[3]);
  return memcmp(&regs[1], "KVMKVMKVM\0\0\0", 12) == 0;
}

#endif

TEST(Threads, ThreadStartInvalidEntry) {
  auto test_thread_start = [&](uintptr_t pc, zx_status_t expected) {
    zx_handle_t process = zx_process_self();
    zx_handle_t thread = ZX_HANDLE_INVALID;
    ASSERT_OK(
        zx_thread_create(process, kThreadName, sizeof(kThreadName) - 1, 0 /* options */, &thread));
    char stack[1024] __ALIGNED(16);  // a small stack for the thread.
    uintptr_t thread_stack = reinterpret_cast<uintptr_t>(&stack[1024]);

    EXPECT_EQ(expected, zx_thread_start(thread, pc, thread_stack, 0 /* arg0 */, 0 /* arg1 */));
    zx_handle_close(thread);
  };

  // This represents an inaccessible address on both aarch64 (because bit 55 == 0
  // indicates an accessible user address) and x86_64 (because the upper 16 bits
  // are not all zero).
  uintptr_t non_user_pc = 0x1UL << 55;
  uintptr_t kernel_pc = 0xffffff8000000000UL;

  test_thread_start(non_user_pc, ZX_ERR_INVALID_ARGS);
  test_thread_start(kernel_pc, ZX_ERR_INVALID_ARGS);

#if defined(__x86_64__)
  uintptr_t non_canonical_pc;
  ASSERT_NO_FATAL_FAILURE(core_test_utils::X86LowestNonCanonicalAddr(non_canonical_pc));
  test_thread_start(non_canonical_pc, ZX_ERR_INVALID_ARGS);
#endif  // defined(__x86_64__)
}

#if defined(__x86_64__)

// Test that zx_thread_write_state() does not allow setting RIP to a
// non-canonical address for a thread that was suspended inside a syscall,
// because if the kernel returns to that address using SYSRET, that can
// cause a fault in kernel mode that is exploitable.  See
// sysret_problem.md.
TEST(Threads, NoncanonicalRipAddressSyscall) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("NoncanonicalRipAddressSyscall"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_wait_fn, event.get()));

  // Wait until the thread has entered the syscall.
  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  zx::suspend_token suspend_token;
  suspend_thread_synchronous(test_thread.thread().get(), suspend_token.reset_and_get_address());

  zx_thread_state_general_regs_t regs;
  ASSERT_OK(test_thread.thread().read_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)));

  // Example addresses to test.
  uintptr_t noncanonical_addr;
  ASSERT_NO_FATAL_FAILURE(core_test_utils::X86LowestNonCanonicalAddr(noncanonical_addr));
  uintptr_t canonical_addr = noncanonical_addr - 1;
  uint64_t kKernelAddr = 0xffffff8000000000UL;

  zx_thread_state_general_regs_t regs_modified = regs;

  // This RIP address must be disallowed.
  regs_modified.rip = noncanonical_addr;
  ASSERT_EQ(test_thread.thread().write_state(ZX_THREAD_STATE_GENERAL_REGS, &regs_modified,
                                             sizeof(regs_modified)),
            ZX_ERR_INVALID_ARGS);

  regs_modified.rip = canonical_addr;
  ASSERT_OK(test_thread.thread().write_state(ZX_THREAD_STATE_GENERAL_REGS, &regs_modified,
                                             sizeof(regs_modified)));

  // This RIP address does not need to be disallowed, but it is currently
  // disallowed because this simplifies the check and it's not useful to
  // allow this address.
  regs_modified.rip = kKernelAddr;
  ASSERT_EQ(test_thread.thread().write_state(ZX_THREAD_STATE_GENERAL_REGS, &regs_modified,
                                             sizeof(regs_modified)),
            ZX_ERR_INVALID_ARGS);

  // Clean up: Restore the original register state.
  ASSERT_OK(test_thread.thread().write_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)));
  // Allow the child thread to resume and exit.
  suspend_token.reset();
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
  // Wait for the child thread to signal that it has continued.
  ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_1, zx::time::infinite(), nullptr));
  // Wait for the child thread to exit.
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

// Test that zx_thread_write_state() does not allow setting RIP to a
// non-canonical address for a thread that was suspended inside an interrupt,
// because if the kernel returns to that address using IRET, that can
// cause a fault in kernel mode that is exploitable.
// See docs/concepts/kernel/sysret_problem.md
TEST(Threads, NoncanonicalRipAddressIRETQ) {
  // Example addresses to test.
  uintptr_t noncanonical_addr;
  ASSERT_NO_FATAL_FAILURE(core_test_utils::X86LowestNonCanonicalAddr(noncanonical_addr));
  uintptr_t kernel_addr = 0xffffff8000000000UL;

  // canonical address that is safe to resume the thread to.
  uintptr_t canonical_addr = reinterpret_cast<uintptr_t>(&spin_address);

  auto test_rip_value = [&](uintptr_t address, zx_status_t expected) {
    zx_thread_state_general_regs_t func_regs;
    RegisterReadSetup<zx_thread_state_general_regs_t> setup;
    setup.RunUntil("NoncanonicalRipAddressIRETQ", &spin_with_general_regs, &func_regs,
                   reinterpret_cast<uintptr_t>(&spin_address));

    zx_thread_state_general_regs_t regs;
    ASSERT_OK(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &regs,
                                   sizeof(regs)));

    regs.rip = address;
    EXPECT_EQ(expected, zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_GENERAL_REGS,
                                              &regs, sizeof(regs)));

    // Resume and re-suspend the thread. Even if the zx_thread_write_state
    // returns an error but sets the registers, we still want to observe the
    // crash. Note that there is no guarantee that it would happen, as the
    // thread might get supsended before it even resumes execution.
    setup.Resume();
    setup.Suspend();
  };

  test_rip_value(canonical_addr, ZX_OK);

  test_rip_value(noncanonical_addr, ZX_ERR_INVALID_ARGS);
  test_rip_value(kernel_addr, ZX_ERR_INVALID_ARGS);
}

#endif  // defined(__x86_64__)

#if defined(__aarch64__)
// Test that, on ARM64, userland cannot use zx_thread_write_state() to
// modify flag bits such as I and F (bits 7 and 6), which are the IRQ and
// FIQ interrupt disable flags.  We don't want userland to be able to set
// those flags to 1, since that would disable interrupts.  Also, userland
// should not be able to read these bits.
TEST(Threads, WritingArmFlagsRegister) {
  std::atomic_int value = 0;
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("WritingArmFlagsRegister"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_atomic_store, &value));
  // Wait for the thread to start executing and enter its main loop.
  while (value.load() != 1) {
    ASSERT_EQ(zx_nanosleep(zx_deadline_after(ZX_USEC(1))), ZX_OK);
  }
  zx::suspend_token suspend_token;
  suspend_thread_synchronous(test_thread.thread().get(), suspend_token.reset_and_get_address());

  zx_thread_state_general_regs_t regs;
  ASSERT_OK(test_thread.thread().read_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)));

  // Check that zx_thread_read_state() does not report any more flag bits
  // than are readable via userland instructions.
  // 0xf0000000 are the NCZV flags
  // 0x00000c00 are the BTYPE flags
  constexpr uint64_t kUserVisibleFlags = 0xf0000c00;
  EXPECT_EQ(regs.cpsr & ~kUserVisibleFlags, 0u);

  // Try setting more flag bits.
  uint64_t original_cpsr = regs.cpsr;
  regs.cpsr |= ~kUserVisibleFlags;
  ASSERT_OK(test_thread.thread().write_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)));

  // Firstly, if we read back the register flag, the extra flag bits
  // should have been ignored and should not be reported as set.
  ASSERT_OK(test_thread.thread().read_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)));
  EXPECT_EQ(regs.cpsr, original_cpsr);

  // Secondly, if we resume the thread, we should be able to kill it.  If
  // zx_thread_write_state() set the interrupt disable flags, then if the
  // thread gets scheduled, it will never get interrupted and we will not
  // be able to kill and join the thread.
  value.store(0);
  suspend_token.reset();
  // Wait until the thread has actually resumed execution.
  while (value.load() != kTestAtomicSetValue) {
    ASSERT_EQ(zx_nanosleep(zx_deadline_after(ZX_USEC(1))), ZX_OK);
  }
  value.store(kTestAtomicExitValue);

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}
#endif

TEST(Threads, WriteReadDebugRegisterState) {
#if defined(__x86_64__)
  zx_thread_state_debug_regs_t debug_regs_to_write;
  zx_thread_state_debug_regs_t debug_regs_expected;
  debug_regs_fill_test_values(&debug_regs_to_write, &debug_regs_expected);

  // Because setting debug state is priviledged, we need to do it through syscalls:
  // 1. Start the thread into a routine that simply spins idly.
  // 2. Suspend it.
  // 3. Write the expected debug state through a syscall.
  // 4. Resume the thread.
  // 5. Suspend it again.
  // 6. Read the state and compare it.

  RegisterReadSetup<zx_thread_state_debug_regs_t> setup;
  setup.RunUntil("WriteReadDebugRegisterState", &spin_with_debug_regs, &debug_regs_to_write,
                 reinterpret_cast<uintptr_t>(&spin_address));

  // Write the test values to the debug registers.
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS,
                                  &debug_regs_to_write, sizeof(debug_regs_to_write)),
            ZX_OK);

  // Resume and re-suspend the thread.
  setup.Resume();
  setup.Suspend();

  // Get the current debug state of the suspended thread.
  zx_thread_state_debug_regs_t regs;
  ASSERT_EQ(
      zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &regs, sizeof(regs)),
      ZX_OK);

  ASSERT_NO_FATAL_FAILURE(debug_regs_expect_eq(__FILE__, __LINE__, regs, debug_regs_expected));

#elif defined(__aarch64__)
  // We get how many breakpoints we have.
  zx_thread_state_debug_regs_t actual_regs = {};
  RegisterReadSetup<zx_thread_state_debug_regs_t> setup;
  setup.RunUntil("WriteReadDebugRegisterState", &spin_with_debug_regs, &actual_regs,
                 reinterpret_cast<uintptr_t>(&spin_address));

  ASSERT_EQ(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &actual_regs,
                                 sizeof(actual_regs)),
            ZX_OK);

  // Arm ensures at least 2 breakpoints.
  ASSERT_GE(actual_regs.hw_bps_count, 2u);
  ASSERT_LE(actual_regs.hw_bps_count, 16u);

  // TODO(donosoc): Once the context switch state tracking is done, add the resume-suspect test
  //                to ensure that it's keeping the state correctly. This is what is done in the
  //                x86 portion of this test.

  zx_thread_state_debug_regs_t regs, expected;
  debug_regs_fill_test_values(&regs, &expected);

  ASSERT_EQ(
      zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &regs, sizeof(regs)),
      ZX_OK);
  ASSERT_EQ(
      zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &regs, sizeof(regs)),
      ZX_OK);

  ASSERT_NO_FATAL_FAILURE(debug_regs_expect_eq(__FILE__, __LINE__, regs, expected));
#endif
}

// All writeable bits as 0.
#define DR6_ZERO_MASK (0xffff0ff0ul)
#define DR7_ZERO_MASK (0x700ul)

TEST(Threads, DebugRegistersValidation) {
#if defined(__x86_64__)
  zx_thread_state_debug_regs_t debug_regs = {};
  RegisterReadSetup<zx_thread_state_debug_regs_t> setup;
  setup.RunUntil("DebugRegistersValidation", &spin_with_debug_regs, &debug_regs,
                 reinterpret_cast<uintptr_t>(&spin_address));

  // Writing all 0s should work and should mask values.
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)),
            ZX_OK);

  ASSERT_EQ(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                 sizeof(debug_regs)),
            ZX_OK);

  for (size_t i = 0; i < 4; i++)
    ASSERT_EQ(debug_regs.dr[i], 0);
  ASSERT_EQ(debug_regs.dr6, DR6_ZERO_MASK);
  ASSERT_EQ(debug_regs.dr7, DR7_ZERO_MASK);

  // Writing an kernel address should fail.
  debug_regs = {};
  debug_regs.dr[2] = 0xffff00000000;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)),
            ZX_ERR_INVALID_ARGS);

  // Invalid values should be masked out.
  debug_regs = {};
  debug_regs.dr6 = ~DR6_ZERO_MASK;
  // We avoid the General Detection flag, which would make us throw an exception on next write.
  debug_regs.dr7 = ~DR7_ZERO_MASK;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)),
            ZX_OK);

  ASSERT_EQ(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                 sizeof(debug_regs)),
            ZX_OK);

  for (size_t i = 0; i < 4; i++)
    ASSERT_EQ(debug_regs.dr[i], 0);
  // Only the user accessible bits of DR6 should be set.
  ASSERT_EQ(debug_regs.dr6, 0xffffefff);
  ASSERT_EQ(debug_regs.dr7, 0xffff07ff);
#elif defined(__aarch64__)
  zx_thread_state_debug_regs_t debug_regs = {};
  zx_thread_state_debug_regs_t actual_regs = {};
  RegisterReadSetup<zx_thread_state_debug_regs_t> setup;
  setup.RunUntil("DebugRegistersValidation", &spin_with_debug_regs, &actual_regs,
                 reinterpret_cast<uintptr_t>(&spin_address));

  // We read the initial state to know how many HW breakpoints we have.
  ASSERT_EQ(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &actual_regs,
                                 sizeof(actual_regs)),
            ZX_OK);

  // Writing a kernel address should fail.
  debug_regs.hw_bps_count = actual_regs.hw_bps_count;
  debug_regs.hw_bps[0].dbgbvr = (uint64_t)-1;
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)),
            ZX_ERR_INVALID_ARGS, "Kernel address should fail");

  // Validation should mask unwanted values from the control register.
  // Only bit 0 is unset. This means the breakpoint is disabled.
  debug_regs.hw_bps[0].dbgbcr = 0xfffffffe;
  debug_regs.hw_bps[0].dbgbvr = 0;  // 0 is a valid value.

  debug_regs.hw_bps[1].dbgbcr = 0x1;  // Only the enabled value is set.
  // We use the address of a function we know is in userspace.
  debug_regs.hw_bps[1].dbgbvr = reinterpret_cast<uint64_t>(wait_thread_blocked);
  ASSERT_EQ(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)),
            ZX_OK, "Validation should correctly mask invalid values");

  // Re-read the state and verify.
  ASSERT_EQ(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &actual_regs,
                                 sizeof(actual_regs)),
            ZX_OK);

  EXPECT_EQ(actual_regs.hw_bps_count, debug_regs.hw_bps_count);
  EXPECT_EQ(actual_regs.hw_bps[0].dbgbcr, 0);
  EXPECT_EQ(actual_regs.hw_bps[0].dbgbvr, 0);
  EXPECT_EQ(actual_regs.hw_bps[1].dbgbcr, 0x000001e5);
  EXPECT_EQ(actual_regs.hw_bps[1].dbgbvr, debug_regs.hw_bps[1].dbgbvr);
#endif
}

#if defined(__x86_64__)
// This is a regression test for https://fxbug.dev/42173827.
//
// See that DR6 is reset after a hardware debug exception.
TEST(Threads, DebugRegistersDr6ResetOnDebugException) {
  // Start a thread that will spin at |spin_address|.
  zx_thread_state_debug_regs_t debug_regs{};
  RegisterReadSetup<zx_thread_state_debug_regs_t> setup;
  setup.RunUntil("DebugRegistersDr6ResetOnDebugException", &spin_with_debug_regs, &debug_regs,
                 reinterpret_cast<uintptr_t>(&spin_address));

  // Create a channel upon which we'll receive debug exceptions.
  zx::channel exception_channel;
  ASSERT_OK(zx::process::self()->create_exception_channel(ZX_EXCEPTION_CHANNEL_DEBUGGER,
                                                          &exception_channel));

  // Set hardware breakpoints, resume the thread, and wait for them to be hit.
  for (size_t i = 0; i < 4; ++i) {
    debug_regs.dr[i] = reinterpret_cast<uintptr_t>(&spin_address);
  }
  debug_regs.dr7 = DR7_ZERO_MASK | 0xff;  // enable "local" and "global" for all 4 with length 1
  ASSERT_OK(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)));
  setup.Resume();
  zx::exception exception;
  wait_thread_excp_type(setup.thread_handle(), exception_channel.get(), ZX_EXCP_HW_BREAKPOINT,
                        ZX_EXCP_THREAD_STARTING, exception.reset_and_get_address());

  // The thread should now be blocked on a debug exception.  Verify the debug register state.
  wait_thread_blocked(setup.thread_handle(), ZX_THREAD_STATE_BLOCKED_EXCEPTION);
  ASSERT_OK(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                 sizeof(debug_regs)));
  for (size_t i = 0; i < 4; ++i) {
    EXPECT_EQ(debug_regs.dr[i], reinterpret_cast<uintptr_t>(&spin_address));
  }

  // See that all 4 breakpoints were hit.
  //
  // TODO(https://fxbug.dev/453746441): Remove this when no longer running on buggy hosts.
  // Due to CVE-2025-21839 we cannot rely on dr6 being accurate if being emulated.
  if (!x86_is_kvm_hypervisor()) {
    EXPECT_EQ(debug_regs.dr6, DR6_ZERO_MASK | 0b1111);
  }

  // Clear the status register and set 4 new breakpoints that should never be hit.
  debug_regs.dr6 = DR6_ZERO_MASK;
  for (size_t i = 0; i < 4; ++i) {
    debug_regs.dr[i] = reinterpret_cast<uintptr_t>(&zx_handle_close);
  }
  debug_regs.dr7 = DR7_ZERO_MASK | 0xff;
  ASSERT_OK(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)));

  // Handle the debug exception and verify the thread resumes execution.
  uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
  ASSERT_OK(exception.set_property(ZX_PROP_EXCEPTION_STATE, &state, sizeof(state)));
  exception.reset();
  zx_signals_t unused = 0u;
  ASSERT_OK(
      zx_object_wait_one(setup.thread_handle(), ZX_THREAD_RUNNING, ZX_TIME_INFINITE, &unused));

  // Suspend the thread and see that the debug registers are as we left them.  In particular, DR6
  // should contain its reset value.
  setup.Suspend();
  ASSERT_OK(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                 sizeof(debug_regs)));
  for (size_t i = 0; i < 4; ++i) {
    EXPECT_EQ(debug_regs.dr[i], reinterpret_cast<uintptr_t>(&zx_handle_close));
  }

  // TODO(https://fxbug.dev/453746441): See comment above.
  if (!x86_is_kvm_hypervisor()) {
    EXPECT_EQ(debug_regs.dr6, DR6_ZERO_MASK);
  }

  EXPECT_EQ(debug_regs.dr7, DR7_ZERO_MASK | 0xff);
}

// This is a regression test for the failure case detailed in https://fxbug.dev/42173827#c8.
//
// See that dr6 remains correct after a hardware breakpoint has been removed.
TEST(Threads, DebugRegistersDr6CorrectAfterBreakpointRemoval) {
  // Start a thread that will spin at |spin_address|.
  zx_thread_state_debug_regs_t debug_regs{};
  RegisterReadSetup<zx_thread_state_debug_regs_t> setup;
  setup.RunUntil("DebugRegistersDr6CorrectAfterBreakpointRemoval", &spin_with_debug_regs,
                 &debug_regs, reinterpret_cast<uintptr_t>(&spin_address));

  // Create a channel upon which we'll receive debug exceptions.
  zx::channel exception_channel;
  ASSERT_OK(zx::process::self()->create_exception_channel(ZX_EXCEPTION_CHANNEL_DEBUGGER,
                                                          &exception_channel));

  // Set hardware breakpoints, resume the thread, and wait for them to be hit.
  for (size_t i = 0; i < 4; ++i) {
    debug_regs.dr[i] = reinterpret_cast<uintptr_t>(&spin_address);
  }
  debug_regs.dr7 = DR7_ZERO_MASK | 0xff;  // enable "local" and "global" for all 4 with length 1
  ASSERT_OK(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)));
  setup.Resume();
  zx::exception exception;
  wait_thread_excp_type(setup.thread_handle(), exception_channel.get(), ZX_EXCP_HW_BREAKPOINT,
                        ZX_EXCP_THREAD_STARTING, exception.reset_and_get_address());

  // The thread should now be blocked on a debug exception. Verify the debug register state.
  wait_thread_blocked(setup.thread_handle(), ZX_THREAD_STATE_BLOCKED_EXCEPTION);
  ASSERT_OK(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                 sizeof(debug_regs)));
  for (size_t i = 0; i < 4; ++i) {
    EXPECT_EQ(debug_regs.dr[i], reinterpret_cast<uintptr_t>(&spin_address));
  }

  zx_thread_state_general_regs_t general_regs{};
  EXPECT_OK(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_GENERAL_REGS, &general_regs,
                                 sizeof(general_regs)));
  EXPECT_EQ(general_regs.rip, reinterpret_cast<uintptr_t>(&spin_address));

  // See that all 4 breakpoints were hit.
  // TODO(https://fxbug.dev/453746441): Remove this when no longer running on older hosts.
  // Due to CVE-2025-21839 we cannot rely on dr6 being accurate if being emulated, but we can at
  // least validate that the value we read now is unchanged over the course of this test.
  const uint64_t orig_dr6 = debug_regs.dr6;
  if (!x86_is_kvm_hypervisor()) {
    EXPECT_EQ(debug_regs.dr6, DR6_ZERO_MASK | 0b1111);
  }

  // Remove the hardware breakpoint.
  for (size_t i = 0; i < 4; ++i) {
    debug_regs.dr[i] = 0;
  }
  debug_regs.dr7 = DR7_ZERO_MASK;
  ASSERT_OK(zx_thread_write_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                  sizeof(debug_regs)));

  // Now read the debug state and verify that dr6 still correctly shows that all 4 breakpoints were
  // hit.
  ASSERT_OK(zx_thread_read_state(setup.thread_handle(), ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                 sizeof(debug_regs)));
  if (x86_is_kvm_hypervisor()) {
    EXPECT_EQ(orig_dr6, debug_regs.dr6);
  } else {
    EXPECT_EQ(debug_regs.dr6, DR6_ZERO_MASK | 0b1111);
  }

  // Handle the exception so that the thread continues.
  uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
  ASSERT_OK(exception.set_property(ZX_PROP_EXCEPTION_STATE, &state, sizeof(state)));
  exception.reset();
}
#endif

// This is a regression test for https://fxbug.dev/42109443. Verify that upon entry to the kernel
// via fault on hardware that lacks SMAP, a subsequent usercopy does not panic.
TEST(Threads, X86AcFlagUserCopy) {
#if defined(__x86_64__)
  zx::process process;
  zx::thread thread;
  zx::event event;
  ASSERT_EQ(zx::event::create(0, &event), ZX_OK);
  ASSERT_EQ(start_mini_process(zx_job_default(), event.get(), process.reset_and_get_address(),
                               thread.reset_and_get_address()),
            ZX_OK);

  // Suspend the process so we can set its AC flag.
  zx::handle suspend_token;
  suspend_thread_synchronous(thread.get(), suspend_token.reset_and_get_address());

  zx_thread_state_general_regs_t regs{};
  ASSERT_EQ(thread.read_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)), ZX_OK);

  // Set AC and change its RIP to 0 so that upon resuming, it will fault and enter the kernel.
  regs.rflags |= (1 << 18);
  regs.rip = 0;
  ASSERT_EQ(thread.write_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)), ZX_OK);

  // We can't catch this exception in userspace, the test requires the
  // kernel do a usercopy from an interrupt context which only happens when
  // the exception falls through unhandled.
  printf("Crashing a test process, the following dump is intentional\n");

  // Resume.
  suspend_token.reset();

  // See that it has terminated.
  ASSERT_EQ(process.wait_one(ZX_THREAD_TERMINATED, zx::time::infinite(), nullptr), ZX_OK);
  zx_info_process_t proc_info{};
  ASSERT_EQ(process.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr),
            ZX_OK);
  ASSERT_EQ(proc_info.return_code, ZX_TASK_RETCODE_EXCEPTION_KILL);
#endif
}

// Verify that syscalls preserve general purpose register state.
//
// When suspended during a syscall, see that argument register read via zx_thread_read_state() match
// the state at the time of the syscall.
TEST(Threads, SyscallSuspendedRegisterState) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Create a thread that waits for a signal then terminates.
  TestThread test_thread;
  syscall_suspended_reg_state_test_arg arg{.event = event.get()};
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SyscallSuspendedRegisterState"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_wait_event_fn, &arg));
  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  // The thread is now blocked in the syscall.  Time to suspend it and read its registers.
  zx::handle suspend_token;
  suspend_thread_synchronous(test_thread.thread().get(), suspend_token.reset_and_get_address());

  zx_thread_state_general_regs_t actual_regs{};
  ASSERT_OK(test_thread.thread().read_state(ZX_THREAD_STATE_GENERAL_REGS, &actual_regs,
                                            sizeof(actual_regs)));

  // We can't verify all the registers since the thread made the call through the vdso, which may
  // have trashed some of them.  We can, however, verify the registers that held the syscall
  // argument.
#if defined(__x86_64__)
  // Note, the 4th syscall argument is passed in r10 rather than rcx.
  EXPECT_EQ(actual_regs.rdi, arg.event);                                  // 1st arg
  EXPECT_EQ(actual_regs.rsi, ZX_USER_SIGNAL_0);                           // 2nd arg
  EXPECT_EQ(actual_regs.rdx, ZX_TIME_INFINITE);                           // 3rd arg
  EXPECT_EQ(actual_regs.r10, reinterpret_cast<uint64_t>(&arg.observed));  // 4th arg
  EXPECT_EQ(actual_regs.rax, ZX_ERR_INTERNAL_INTR_RETRY);                 // syscall result
#elif defined(__aarch64__)
  // We can't check the 1st arg because x0 is also used to store the result.
  EXPECT_EQ(actual_regs.r[1], ZX_USER_SIGNAL_0);                           // 2nd arg
  EXPECT_EQ(actual_regs.r[2], ZX_TIME_INFINITE);                           // 3rd arg
  EXPECT_EQ(actual_regs.r[3], reinterpret_cast<uint64_t>(&arg.observed));  // 4th arg
  EXPECT_EQ(actual_regs.r[0], ZX_ERR_INTERNAL_INTR_RETRY);                 // syscall result
#elif defined(__riscv)
  // We can't check the 1st arg because a0 is also used to store the result.
  EXPECT_EQ(actual_regs.a1, ZX_USER_SIGNAL_0);                           // 2nd arg
  EXPECT_EQ(actual_regs.a2, ZX_TIME_INFINITE);                           // 3rd arg
  EXPECT_EQ(actual_regs.a3, reinterpret_cast<uint64_t>(&arg.observed));  // 4th arg
  EXPECT_EQ(actual_regs.a0, ZX_ERR_INTERNAL_INTR_RETRY);                 // syscall result
#else
#error unsupported platform
#endif

  suspend_token.reset();
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
}

// Verify that a debugger can modify the syscall result of a suspended thread.
TEST(Threads, SyscallDebuggerModifyResult) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Create a thread that waits for a signal then terminates.
  syscall_suspended_reg_state_test_arg arg{.event = event.get()};
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("SyscallDebuggerModifyResult"));
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_wait_event_fn, &arg));
  wait_thread_blocked(test_thread.thread().get(), ZX_THREAD_STATE_BLOCKED_WAIT_ONE);

  // The thread is now blocked in the syscall.  Time to suspend it and read its registers.
  zx::handle suspend_token;
  suspend_thread_synchronous(test_thread.thread().get(), suspend_token.reset_and_get_address());
  zx_thread_state_general_regs_t actual_regs{};
  ASSERT_OK(test_thread.thread().read_state(ZX_THREAD_STATE_GENERAL_REGS, &actual_regs,
                                            sizeof(actual_regs)));

  // Change the syscall result to ZX_ERR_CANCELED.
#if defined(__x86_64__)
  EXPECT_EQ(actual_regs.rax, ZX_ERR_INTERNAL_INTR_RETRY);
  actual_regs.rax = ZX_ERR_CANCELED;
#elif defined(__aarch64__)
  EXPECT_EQ(actual_regs.r[0], ZX_ERR_INTERNAL_INTR_RETRY);
  actual_regs.r[0] = ZX_ERR_CANCELED;
#elif defined(__riscv)
  EXPECT_EQ(actual_regs.a0, ZX_ERR_INTERNAL_INTR_RETRY);
  actual_regs.a0 = ZX_ERR_CANCELED;
#else
#error unsupported platform
#endif
  ASSERT_OK(test_thread.thread().write_state(ZX_THREAD_STATE_GENERAL_REGS, &actual_regs,
                                             sizeof(actual_regs)));

  // Resume and see that the syscall did complete with ZX_ERR_CANCELED.
  suspend_token.reset();
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_0));
  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());
  ASSERT_EQ(arg.status, ZX_ERR_CANCELED);
}

TEST(Threads, YieldWithZeroOptionIsOk) { ASSERT_OK(zx_thread_legacy_yield(0ull)); }

TEST(Threads, YieldWithNonZeroOptionIsInvalidArgs) {
  ASSERT_STATUS(zx_thread_legacy_yield(std::numeric_limits<uint32_t>::max()), ZX_ERR_INVALID_ARGS);
}

// The StartRegs* tests use an assembly snippet that stores register values
// onto its tiny stack, which is represented by this struct.
struct alignas(16) StartRegsEntryStack {
  char call_stack[256];  // Stack space for the zx_thread_exit() system call.
  struct Regs {
    uint64_t arg0 = 0;
    uint64_t arg1 = 0;
    uint64_t tp = 0;
    uint64_t abi_reg = 0;
  } regs;
};
static_assert(sizeof(StartRegsEntryStack::Regs) == 32);

struct StartRegsFromState {
  explicit StartRegsFromState(const zx_thread_state_general_regs_t& state)
      :
#ifdef __x86_64__
        pc{state.rip},
        sp{state.rsp},
#else
        pc{state.pc},
        sp{state.sp},
#endif
        regs{
#if defined(__aarch64__)
            .arg0 = state.r[0],
            .arg1 = state.r[1],
            .tp = state.tpidr,
            .abi_reg = state.r[18],
#elif defined(__riscv)
            .arg0 = state.a0,
            .arg1 = state.a1,
            .tp = state.tp,
            .abi_reg = state.gp,
#elif defined(__x86_64__)
            .arg0 = state.rdi,
            .arg1 = state.rsi,
            .tp = state.fs_base,
            .abi_reg = state.r15,
#else
#error unsupported platform
#endif
        } {
  }

  uint64_t pc, sp;
  StartRegsEntryStack::Regs regs;
};

// Actually defined in asm below.  This code simply stores the four register
// values passed through the kernel onto the stack in the layout above.  On
// entry, the SP points past the end of the StartRegsEntryStack structure.
extern "C" void StartRegsEntry() [[clang::cfi_unchecked_callee]];

constexpr uint64_t kArg0Value = 0x1234567890abcdef;
constexpr uint64_t kArg1Value = 0xabcdef1234567890;
constexpr uint64_t kAbiRegValue = 0xfeedfacedeadbeef;

__asm__(
    R"""(
    .pushsection .text.StartRegsEntry, "ax", %progbits
    .type StartRegsEntry, %function
    StartRegsEntry:
      .cfi_startproc
    )"""
#if defined(__aarch64__)
    R"""(
      mrs x17, TPIDR_EL0
      stp x0, x1, [sp, #-32]!
      .cfi_adjust_cfa_offset 32
      stp x17, x18, [sp, #16]
      bl _zx_thread_exit
      udf #0
    )"""
#elif defined(__riscv)
    R"""(
      add sp, sp, -32
      .cfi_adjust_cfa_offset 32
      sd a0, 0(sp)
      sd a1, 8(sp)
      sd tp, 16(sp)
      sd gp, 24(sp)
      tail _zx_thread_exit
      unimp
    )"""
#elif defined(__x86_64__)
    R"""(
      push %r15
      .cfi_adjust_cfa_offset 8
      push %fs:0
      .cfi_adjust_cfa_offset 8
      push %rsi
      .cfi_adjust_cfa_offset 8
      push %rdi
      .cfi_adjust_cfa_offset 8
      call _zx_thread_exit@PLT
      ud2
    )"""
#else
#error unsupported platform
#endif
    R"""(
      .cfi_endproc
    .size StartRegsEntry, . - StartRegsEntry
    .popsection
    )""");

TEST(Threads, StartRegs) {
  zx::thread thread;
  constexpr std::string_view kName = "Threads.StartRegs test";
  ASSERT_OK(zx::thread::create(*zx::process::self(), kName.data(), kName.size(), 0, &thread));

  const uint64_t pc = reinterpret_cast<uintptr_t>(&StartRegsEntry);

  StartRegsEntryStack stack;
  const uint64_t sp = reinterpret_cast<uintptr_t>(&stack + 1);

  // On x86, `rdfsbase` isn't necessarily available, so rather than directly
  // reading the `%fs.base` value, the test code just reads `%fs:0` where its
  // own address is stored (as in the real ABI).
  const uint64_t tp_value = reinterpret_cast<uintptr_t>(&tp_value);

  // Start the thread with the chosen register values.
  ASSERT_OK(
      zx_thread_start_regs(thread.get(), pc, sp, kArg0Value, kArg1Value, tp_value, kAbiRegValue));

  // Wait for it to exit after storing values in stack.regs.
  zx_signals_t signals = 0;
  ASSERT_OK(thread.wait_one(ZX_THREAD_TERMINATED, zx::time::infinite(), &signals));
  ASSERT_TRUE(signals & ZX_THREAD_TERMINATED);

  EXPECT_EQ(stack.regs.arg0, kArg0Value);
  EXPECT_EQ(stack.regs.arg1, kArg1Value);
  EXPECT_EQ(stack.regs.tp, tp_value);
  EXPECT_EQ(stack.regs.abi_reg, kAbiRegValue);
}

TEST(Threads, StartRegsSuspended) {
  zx::thread thread;
  constexpr std::string_view kName = "Threads.StartRegsSuspended test";
  ASSERT_OK(zx::thread::create(*zx::process::self(), kName.data(), kName.size(), 0, &thread));

  // Suspend the thread before it's started.
  zx::suspend_token token;
  ASSERT_OK(thread.suspend(&token));

  // This will actually execute, but only after the test is done and just so
  // the thread can reach its zx_thread_exit() system call since there is no
  // way to kill the thread directly.
  const uint64_t pc = reinterpret_cast<uintptr_t>(&StartRegsEntry);

  // This needs to be a usable stack where the assembly code can do its stores
  // and then make the system call.  But it won't be inspected later.
  StartRegsEntryStack stack;
  const uint64_t sp = reinterpret_cast<uintptr_t>(&stack + 1);

  // This needs to be a readable address for the x86-64 assembly code.
  const uint64_t tp_value = reinterpret_cast<uintptr_t>(&tp_value);

  // Start the thread with the chosen register values.
  ASSERT_OK(
      zx_thread_start_regs(thread.get(), pc, sp, kArg0Value, kArg1Value, tp_value, kAbiRegValue));

  // Wait for the kernel thread to start and report itself suspended before
  // actually going to user mode.
  zx_signals_t signals = 0;
  ASSERT_OK(
      thread.wait_one(ZX_THREAD_TERMINATED | ZX_THREAD_SUSPENDED, zx::time::infinite(), &signals));
  ASSERT_TRUE(signals & ZX_THREAD_SUSPENDED);

  zx_thread_state_general_regs_t state;
  ASSERT_OK(thread.read_state(ZX_THREAD_STATE_GENERAL_REGS, &state, sizeof(state)));

  StartRegsFromState start{state};
  EXPECT_EQ(start.pc, pc);
  EXPECT_EQ(start.sp, sp);
  EXPECT_EQ(start.regs.arg0, kArg0Value);
  EXPECT_EQ(start.regs.arg1, kArg1Value);
  EXPECT_EQ(start.regs.tp, tp_value);
  EXPECT_EQ(start.regs.abi_reg, kAbiRegValue);

  // Let the thread resume so it will exit.  Then wait for it to finish exiting
  // so that its stack (which is in a local variable in this function) is no
  // longer being touched before this function returns.
  token.reset();
  ASSERT_OK(thread.wait_one(ZX_THREAD_TERMINATED, zx::time::infinite(), &signals));
  ASSERT_TRUE(signals & ZX_THREAD_TERMINATED);
}

#ifdef __x86_64__
TEST(Threads, StartRegsNoncanonicalTp) {
  zx::thread thread;
  constexpr std::string_view kName = "Threads.StartRegsNoncanonicalTp";
  ASSERT_OK(zx::thread::create(*zx::process::self(), kName.data(), kName.size(), 0, &thread));

  uintptr_t non_canonical_fsbase;
  ASSERT_NO_FATAL_FAILURE(core_test_utils::X86LowestNonCanonicalAddr(non_canonical_fsbase));

  ASSERT_EQ(zx_thread_start_regs(thread.get(), 0, 0, 0, 0, non_canonical_fsbase, 0),
            ZX_ERR_INVALID_ARGS);
}

TEST(Threads, DebugRegistersWatchpointSyscallPanicRepro) {
  // Create a VMO we are going to ask the kernel to read from.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Memory location we are going to set watchpoint on and ask the kernel to access.
  uint64_t watched_value = 42;

  std::thread t([&]() {
    // Signal that we are here.
    EXPECT_OK(event.signal(0, ZX_USER_SIGNAL_0));
    // Wait for the signal that the watchpoint has been setup.
    EXPECT_OK(event.wait_one(ZX_USER_SIGNAL_1, zx::time::infinite(), nullptr));

    // Read into the location where the watchpoint is, this will cause the kernel to be the one to
    // access the address and potentially trigger the watchpoint.
    EXPECT_OK(vmo.read(&watched_value, 0, 8));
    // Verify that the kernel wrote the value zero into it. This should also trigger the watchpoint,
    // but this time give us a debug exception.
    EXPECT_EQ(watched_value, 0u);
  });

  // Wait for the thread to be ready.
  EXPECT_OK(event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));

  // Create a channel upon which we'll receive debug exceptions.
  zx::channel exception_channel;
  ASSERT_OK(zx::process::self()->create_exception_channel(ZX_EXCEPTION_CHANNEL_DEBUGGER,
                                                          &exception_channel));

  // Now suspend it so we can modify the debug state.
  zx::suspend_token suspend;
  zx::unowned<zx::thread> thread =
      zx::unowned<zx::thread>(native_thread_get_zx_handle(t.native_handle()));
  EXPECT_OK(thread->suspend(&suspend));
  EXPECT_OK(thread->wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), nullptr));

  // Set debug registers
  zx_thread_state_debug_regs_t debug_regs = {};
  debug_regs.dr[0] = reinterpret_cast<uint64_t>(&watched_value);
  // RW0 = 01 (write/write), LEN0 = 00 (1 byte), L0 = 1
  debug_regs.dr7 = DR7_ZERO_MASK | (0b11ul << 16) | (0ul << 18) | 1ul;
  EXPECT_OK(thread->write_state(ZX_THREAD_STATE_DEBUG_REGS, &debug_regs, sizeof(debug_regs)));

  // Resume and signal that the thread can proceed with the read.
  suspend.reset();
  EXPECT_OK(event.signal(0, ZX_USER_SIGNAL_1));

  // Wait for the thread to trigger the watchpoint.
  zx::exception exception;
  wait_thread_excp_type(thread->get(), exception_channel.get(), ZX_EXCP_HW_BREAKPOINT, 0,
                        exception.reset_and_get_address());

  // Clear the watchpoint before resuming.
  debug_regs.dr7 = DR7_ZERO_MASK;
  EXPECT_OK(thread->write_state(ZX_THREAD_STATE_DEBUG_REGS, &debug_regs, sizeof(debug_regs)));

  // Handle the exception so that the thread continues.
  uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
  ASSERT_OK(exception.set_property(ZX_PROP_EXCEPTION_STATE, &state, sizeof(state)));
  exception.reset();

  t.join();
}

TEST(Threads, WatchpointSyscallPanic) {
  zx::debuglog dlog;
  ASSERT_OK(zx::debuglog::create(zx::resource(), 0, &dlog));

  const size_t page_size = zx_system_get_page_size();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(page_size, 0, &vmo));
  zx_vaddr_t addr;
  ASSERT_OK(
      zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, page_size, &addr));

  zx::vmo stack_vmo;
  ASSERT_OK(zx::vmo::create(page_size, 0, &stack_vmo));
  zx_vaddr_t stack_addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, stack_vmo, 0,
                                       page_size, &stack_addr));
  zx_vaddr_t sp = stack_addr + page_size;

  sp -= 8;
  *reinterpret_cast<zx_vaddr_t*>(sp) = reinterpret_cast<zx_vaddr_t>(&zx_thread_exit);

  zx::thread thread;
  ASSERT_OK(zx::thread::create(*zx::process::self(), "watchpoint-thread", 17, 0, &thread));

  zx::suspend_token suspend_token;
  ASSERT_OK(thread.suspend(&suspend_token));

  zx_vaddr_t pc = reinterpret_cast<zx_vaddr_t>(&zx_debuglog_write);
  ASSERT_OK(thread.start(pc, sp, 0, 0));

  zx_signals_t observed;
  ASSERT_OK(thread.wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), &observed));

  zx_thread_state_general_regs_t gregs;
  ASSERT_OK(thread.read_state(ZX_THREAD_STATE_GENERAL_REGS, &gregs, sizeof(gregs)));

  gregs.rdi = dlog.get();
  gregs.rsi = 0;
  gregs.rdx = addr;
  gregs.rcx = 1;

  ASSERT_OK(thread.write_state(ZX_THREAD_STATE_GENERAL_REGS, &gregs, sizeof(gregs)));

  zx_thread_state_debug_regs_t dregs;
  memset(&dregs, 0, sizeof(dregs));

  dregs.dr[0] = addr;
  dregs.dr7 = DR7_ZERO_MASK | 1ul | (3ul << 16) | (3ul << 18);
  dregs.dr6 = DR6_ZERO_MASK | (1ul << 14);  // Set BS bit

  ASSERT_OK(thread.write_state(ZX_THREAD_STATE_DEBUG_REGS, &dregs, sizeof(dregs)));

  suspend_token.reset();

  zx::nanosleep(zx::deadline_after(zx::sec(1)));

  zx_task_kill(thread.get());
  thread.wait_one(ZX_THREAD_TERMINATED, zx::time::infinite(), nullptr);
}
#endif

}  // namespace
