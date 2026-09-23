// Copyright 2017 The Fuchsia Authors. All rights reserved.
//
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thread-functions.h"

#include <zircon/syscalls.h>
#include <zircon/syscalls/port.h>

#include <atomic>
#include <cstring>

void threads_test_sleep_fn(zx_instant_mono_t time) {
  zx_nanosleep(time);
  zx_thread_exit();
}

void threads_test_wait_fn(zx_handle_t event) {
  zx_object_wait_one(event, ZX_USER_SIGNAL_0, ZX_TIME_INFINITE, nullptr);
  zx_object_signal(event, 0u, ZX_USER_SIGNAL_1);
  zx_thread_exit();
}

void threads_test_wait_break_fn(zx_handle_t event) {
  zx_object_wait_one(event, ZX_USER_SIGNAL_0, ZX_TIME_INFINITE, nullptr);

  // Don't use builtin_trap since the compiler might assume everything after that call can't
  // execute and might remove the function epilog. The test harness will catch the exception
  // and step over it.
#if __has_builtin(__builtin_debugtrap)
  __builtin_debugtrap();
#elif defined(__aarch64__)
  __asm__ volatile("brk 0");
#elif defined(__riscv)
  __asm__ volatile("ebreak");
#elif defined(__x86_64__)
  __asm__ volatile("int3");
#else
#error Not supported on this platform.
#endif
  zx_thread_exit();
}

void threads_test_infinite_wait_fn(zx_handle_t event) {
  zx_object_wait_one(event, ZX_USER_SIGNAL_0, ZX_TIME_INFINITE, nullptr);
  __builtin_trap();
}

void threads_test_port_fn(zx_handle_t port[2]) {
  zx_port_packet_t packet = {};
  zx_port_wait(port[0], ZX_TIME_INFINITE, &packet);
  packet.key += 5u;
  zx_port_queue(port[1], &packet);
  zx_thread_exit();
}

void threads_test_channel_call_fn(channel_call_suspend_test_arg* arg) {
  uint8_t send_buf[9] = {'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i'};
  uint8_t recv_buf[9];
  uint32_t actual_bytes, actual_handles;

  zx_channel_call_args_t call_args = {
      .wr_bytes = send_buf,
      .wr_handles = nullptr,
      .rd_bytes = recv_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(send_buf),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(recv_buf),
      .rd_num_handles = 0,
  };

  arg->call_status = zx_channel_call(arg->channel, 0, ZX_TIME_INFINITE, &call_args, &actual_bytes,
                                     &actual_handles);
  if (arg->call_status == ZX_OK) {
    if (actual_bytes != sizeof(recv_buf) ||
        memcmp(recv_buf + sizeof(zx_txid_t), &"abcdefghj"[sizeof(zx_txid_t)],
               sizeof(recv_buf) - sizeof(zx_txid_t)) != 0) {
      arg->call_status = ZX_ERR_BAD_STATE;
    }
  }

  zx_handle_close(arg->channel);
  zx_thread_exit();
}

void threads_bad_syscall_fn(const bad_syscall_arg* arg) {
  zx_object_wait_one(arg->event, ZX_USER_SIGNAL_0, ZX_TIME_INFINITE, nullptr);
  uint64_t syscall_number = arg->syscall_number;
#if defined(__aarch64__)
  __asm__ volatile(
      "mov x16, %0\n"
      "svc #0"
      :
      : "r"(syscall_number)
      : "x16");
#elif defined(__riscv)
  __asm__ volatile(
      "mv t0, %0\n"
      "ecall"
      :
      : "r"(syscall_number)
      : "t0");
#elif defined(__x86_64__)
  __asm__ volatile("syscall" : : "a"(syscall_number));
#else
#error Not supported on this platform.
#endif
  zx_thread_exit();
}

void threads_test_atomic_store(std::atomic_int* p) {
  while (atomic_exchange(p, kTestAtomicSetValue) != kTestAtomicExitValue) {
  }
  zx_thread_exit();
}

void threads_test_run_fn(zx_handle_t event) {
  zx_object_signal(event, 0u, ZX_USER_SIGNAL_0);
  zx_object_wait_one(event, ZX_USER_SIGNAL_1, ZX_TIME_INFINITE, nullptr);
  zx_thread_exit();
}

void threads_test_wait_event_fn(syscall_suspended_reg_state_test_arg* arg) {
  arg->status = zx_object_wait_one(arg->event, ZX_USER_SIGNAL_0, ZX_TIME_INFINITE, &arg->observed);
  zx_thread_exit();
}

void threads_test_wait_loop(zx_handle_t event) {
  zx_object_signal(event, 0u, ZX_USER_SIGNAL_0);
  for (;;) {
    zx_status_t wait_result =
        zx_object_wait_one(event, ZX_USER_SIGNAL_1, ZX_TIME_INFINITE_PAST, nullptr);
    if (wait_result != ZX_ERR_TIMED_OUT) {
      break;
    }
  }
  zx_thread_exit();
}
