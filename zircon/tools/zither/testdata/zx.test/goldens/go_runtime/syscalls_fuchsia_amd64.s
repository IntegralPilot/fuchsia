// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

#include "textflag.h"


// func Sys_channel_read(handle Handle, options uint32, bytes unsafe.Pointer, handles *Handle, num_bytes uint32, num_handles uint32, actual_bytes *uint32, actual_handles *uint32) Status
TEXT ·Sys_channel_read(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_channel_read(SB)

// func Sys_channel_write(handle Handle, options uint32, bytes unsafe.Pointer, num_bytes uint32, handles *Handle, num_handles uint32) Status
TEXT ·Sys_channel_write(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_channel_write(SB)

// func Sys_clock_get_monotonic() Time
TEXT ·Sys_clock_get_monotonic(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_clock_get_monotonic(SB)

// func Sys_clock_get_monotonic_via_kernel() Time
TEXT ·Sys_clock_get_monotonic_via_kernel(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_clock_get_monotonic_via_kernel(SB)

// func Sys_handle_close_many(handles *Handle, num_handles uint) Status
TEXT ·Sys_handle_close_many(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_handle_close_many(SB)

// func Sys_ktrace_control(handle Handle, action uint32, options uint32, ptr unsafe.Pointer) Status
TEXT ·Sys_ktrace_control(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_ktrace_control(SB)

// func Sys_nanosleep(deadline Time) Status
TEXT ·Sys_nanosleep(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_nanosleep(SB)

// func Sys_process_exit(retcode int64)
TEXT ·Sys_process_exit(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_process_exit(SB)

// func Sys_syscall_next()
TEXT ·Sys_syscall_next(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_syscall_next(SB)

// func Sys_syscall_test0()
TEXT ·Sys_syscall_test0(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_syscall_test0(SB)

// func Sys_syscall_test1()
TEXT ·Sys_syscall_test1(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_syscall_test1(SB)

// func Sys_syscall_test2()
TEXT ·Sys_syscall_test2(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_syscall_test2(SB)

// func Sys_system_get_page_size() uint32
TEXT ·Sys_system_get_page_size(SB),NOSPLIT,$0
	JMP runtime·vdsoCall_zx_system_get_page_size(SB)
