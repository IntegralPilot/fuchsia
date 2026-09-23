// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

package zx

import "unsafe"

//go:noescape
//go:nosplit
func Sys_channel_read(handle Handle, options uint32, bytes unsafe.Pointer, handles *Handle, num_bytes uint32, num_handles uint32, actual_bytes *uint32, actual_handles *uint32) Status

//go:noescape
//go:nosplit
func Sys_channel_write(handle Handle, options uint32, bytes unsafe.Pointer, num_bytes uint32, handles *Handle, num_handles uint32) Status

//go:noescape
//go:nosplit
func Sys_clock_get_monotonic() Time

//go:noescape
//go:nosplit
func Sys_clock_get_monotonic_via_kernel() Time

//go:noescape
//go:nosplit
func Sys_handle_close_many(handles *Handle, num_handles uint) Status

//go:noescape
//go:nosplit
func Sys_ktrace_control(handle Handle, action uint32, options uint32, ptr unsafe.Pointer) Status

//go:noescape
//go:nosplit
func Sys_nanosleep(deadline Time) Status

//go:noescape
//go:nosplit
func Sys_process_exit(retcode int64)

//go:noescape
//go:nosplit
func Sys_syscall_next()

//go:noescape
//go:nosplit
func Sys_syscall_test0()

//go:noescape
//go:nosplit
func Sys_syscall_test1()

//go:noescape
//go:nosplit
func Sys_syscall_test2()

//go:noescape
//go:nosplit
func Sys_system_get_page_size() uint32
