// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

#ifndef LIB_SYSCALLS_ZX_SYSCALL_NUMBERS_H_
#define LIB_SYSCALLS_ZX_SYSCALL_NUMBERS_H_

#define ZX_SYS_channel_read 0
#define ZX_SYS_channel_write 1
#define ZX_SYS_clock_get_monotonic_via_kernel 2
#define ZX_SYS_handle_close_many 3
#define ZX_SYS_ktrace_control 4
#define ZX_SYS_nanosleep 5
#define ZX_SYS_process_exit 6
#define ZX_SYS_syscall_next 7
#define ZX_SYS_syscall_test0 8
#define ZX_SYS_syscall_test1 9
#define ZX_SYS_syscall_test2 10
#define ZX_SYS_COUNT 11

#ifndef __ASSEMBLER__

// Indexed by syscall number.
inline constexpr const char* kSyscallNames[] = {
    "zx_channel_read",      "zx_channel_write",  "zx_clock_get_monotonic_via_kernel",
    "zx_handle_close_many", "zx_ktrace_control", "zx_nanosleep",
    "zx_process_exit",      "zx_syscall_next",   "zx_syscall_test0",
    "zx_syscall_test1",     "zx_syscall_test2",
};

#endif  // #ifndef __ASSEMBLER__

#endif  // LIB_SYSCALLS_ZX_SYSCALL_NUMBERS_H_
