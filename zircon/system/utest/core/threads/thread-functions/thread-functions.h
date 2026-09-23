// Copyright 2017 The Fuchsia Authors. All rights reserved.
//
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_THREADS_THREAD_FUNCTIONS_THREAD_FUNCTIONS_H_
#define ZIRCON_SYSTEM_UTEST_CORE_THREADS_THREAD_FUNCTIONS_THREAD_FUNCTIONS_H_

#include <zircon/types.h>

#include <atomic>

// This file contains thread functions that do various things useful for
// testing thread behavior.  These functions are built in isolation using only
// the basic machine ABI and vDSO calls.  All are [[noreturn]] and end with
// calling zx_thread_exit().  They are meant to be run via TestThread objects.
// The symbols exposed across the hermetic isolation barrier must be extern "C"
// and listed in the build rules.

constexpr int kTestAtomicSetValue = 1;
constexpr int kTestAtomicExitValue = 2;
constexpr int kTestAtomicClobberValue = 3;

extern "C" {

// The arg is a zx_instant_mono_t which is passed to zx_nanosleep.
[[noreturn]] void threads_test_sleep_fn(zx_instant_mono_t time);

// The arg is an event. It will first be waited on for signal 0, then it will issue signal 1 to
// notify completion.
[[noreturn]] void threads_test_wait_fn(zx_handle_t event);

// The arg is an event which will be waited on for signal 0 (to synchronize the beginning), then
// it will issue a debug break instruction (causing a SW_BREAKPOINT exception), then it will exit.
[[noreturn]] void threads_test_wait_break_fn(zx_handle_t event);

// This thread issues an infinite wait on signal 0 of the event whose handle is passed in arg.
[[noreturn]] void threads_test_infinite_wait_fn(zx_handle_t event);

// The arg is a port handle which is waited on. When a packet is received, it will send a packet
// to the port whose key is 5 greater than the input key.
[[noreturn]] void threads_test_port_fn(zx_handle_t port[2]);

// The arg is a pointer to channel_call_suspend_test_arg (below). The function will send a small
// message and expects to receive the same contents in a reply.
//
// On completion, arg->call_status will be set to the success of the operation.
struct channel_call_suspend_test_arg {
  zx_handle_t channel;
  zx_status_t call_status;
};
[[noreturn]] void threads_test_channel_call_fn(channel_call_suspend_test_arg* arg);

// The arg is a pointer to bad_syscall_arg (below). The function will wait for ZX_USER_SIGNAL_0
// on the given event and then issue the given (bad) syscall.
struct bad_syscall_arg {
  zx_handle_t event;
  uint64_t syscall_number;
};
[[noreturn]] void threads_bad_syscall_fn(const bad_syscall_arg*);

// The function loops storing |kTestAtomicSetValue| there until it sees
// |kTestAtomicExitValue| then exits.
[[noreturn]] void threads_test_atomic_store(std::atomic_int*);

// The arg is an event. It will first send a signal 0 to indicate begin running then wiat for a
// signal 1 to stop running.
[[noreturn]] void threads_test_run_fn(zx_handle_t event);

struct syscall_suspended_reg_state_test_arg {
  zx_handle_t event;
  zx_signals_t observed;
  zx_status_t status;
};

// Waits on |event| for ZX_USER_SIGNAL_0, stores the observed signals in |observed|, stores the
// syscall result in |status|.
//
// |arg| is a syscall_suspended_reg_state_test_arg.
[[noreturn]] void threads_test_wait_event_fn(syscall_suspended_reg_state_test_arg* arg);

// The arg is an event. It will first issue signal 0, then it will continuously check for signal 1.
// On finding signal 1, it will return.
[[noreturn]] void threads_test_wait_loop(zx_handle_t event);

}  // extern "C"

#endif  // ZIRCON_SYSTEM_UTEST_CORE_THREADS_THREAD_FUNCTIONS_THREAD_FUNCTIONS_H_
