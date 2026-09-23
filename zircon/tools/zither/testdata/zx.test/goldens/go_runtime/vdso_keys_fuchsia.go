// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

package runtime

import "unsafe"

const (
	// vdsoArrayMax is the byte-size of a maximally sized array on this architecture.
	// See cmd/compile/internal/amd64/galign.go arch.MAXWIDTH initialization.
	vdsoArrayMax = 1<<50 - 1
)

var vdsoSymbolKeys = []vdsoSymbolKey{
	{"_zx_channel_read", 0xe7169b09, &vdso_zx_channel_read},
	{"_zx_channel_write", 0xca4bbc18, &vdso_zx_channel_write},
	{"_zx_clock_get_monotonic", 0xb00e6115, &vdso_zx_clock_get_monotonic},
	{"_zx_clock_get_monotonic_via_kernel", 0x3dc12b54, &vdso_zx_clock_get_monotonic_via_kernel},
	{"_zx_handle_close_many", 0x8a9a3aaa, &vdso_zx_handle_close_many},
	{"_zx_ktrace_control", 0x15debecf, &vdso_zx_ktrace_control},
	{"_zx_nanosleep", 0xe9d6145a, &vdso_zx_nanosleep},
	{"_zx_process_exit", 0xc7f8a64d, &vdso_zx_process_exit},
	{"_zx_syscall_next", 0x34577f0e, &vdso_zx_syscall_next},
	{"_zx_syscall_test0", 0xbfb3debf, &vdso_zx_syscall_test0},
	{"_zx_syscall_test1", 0xbfb3dec0, &vdso_zx_syscall_test1},
	{"_zx_syscall_test2", 0xbfb3dec1, &vdso_zx_syscall_test2},
	{"_zx_system_get_page_size", 0x1495920f, &vdso_zx_system_get_page_size},
}

//go:cgo_import_dynamic vdso_zx_channel_read zx_channel_read
//go:cgo_import_dynamic vdso_zx_channel_write zx_channel_write
//go:cgo_import_dynamic vdso_zx_clock_get_monotonic zx_clock_get_monotonic
//go:cgo_import_dynamic vdso_zx_clock_get_monotonic_via_kernel zx_clock_get_monotonic_via_kernel
//go:cgo_import_dynamic vdso_zx_handle_close_many zx_handle_close_many
//go:cgo_import_dynamic vdso_zx_ktrace_control zx_ktrace_control
//go:cgo_import_dynamic vdso_zx_nanosleep zx_nanosleep
//go:cgo_import_dynamic vdso_zx_process_exit zx_process_exit
//go:cgo_import_dynamic vdso_zx_syscall_next zx_syscall_next
//go:cgo_import_dynamic vdso_zx_syscall_test0 zx_syscall_test0
//go:cgo_import_dynamic vdso_zx_syscall_test1 zx_syscall_test1
//go:cgo_import_dynamic vdso_zx_syscall_test2 zx_syscall_test2
//go:cgo_import_dynamic vdso_zx_system_get_page_size zx_system_get_page_size

//go:linkname vdso_zx_channel_read vdso_zx_channel_read
//go:linkname vdso_zx_channel_write vdso_zx_channel_write
//go:linkname vdso_zx_clock_get_monotonic vdso_zx_clock_get_monotonic
//go:linkname vdso_zx_clock_get_monotonic_via_kernel vdso_zx_clock_get_monotonic_via_kernel
//go:linkname vdso_zx_handle_close_many vdso_zx_handle_close_many
//go:linkname vdso_zx_ktrace_control vdso_zx_ktrace_control
//go:linkname vdso_zx_nanosleep vdso_zx_nanosleep
//go:linkname vdso_zx_process_exit vdso_zx_process_exit
//go:linkname vdso_zx_syscall_next vdso_zx_syscall_next
//go:linkname vdso_zx_syscall_test0 vdso_zx_syscall_test0
//go:linkname vdso_zx_syscall_test1 vdso_zx_syscall_test1
//go:linkname vdso_zx_syscall_test2 vdso_zx_syscall_test2
//go:linkname vdso_zx_system_get_page_size vdso_zx_system_get_page_size

//go:noescape
//go:nosplit
func vdsoCall_zx_channel_read(handle uint32, options uint32, bytes unsafe.Pointer, handles unsafe.Pointer, num_bytes uint32, num_handles uint32, actual_bytes unsafe.Pointer, actual_handles unsafe.Pointer) int32

//go:noescape
//go:nosplit
func vdsoCall_zx_channel_write(handle uint32, options uint32, bytes unsafe.Pointer, num_bytes uint32, handles unsafe.Pointer, num_handles uint32) int32

//go:noescape
//go:nosplit
func vdsoCall_zx_clock_get_monotonic() uint64

//go:noescape
//go:nosplit
func vdsoCall_zx_clock_get_monotonic_via_kernel() uint64

//go:noescape
//go:nosplit
func vdsoCall_zx_handle_close_many(handles unsafe.Pointer, num_handles uint) int32

//go:noescape
//go:nosplit
func vdsoCall_zx_ktrace_control(handle uint32, action uint32, options uint32, ptr unsafe.Pointer) int32

//go:noescape
//go:nosplit
func vdsoCall_zx_nanosleep(deadline uint64) int32

//go:noescape
//go:nosplit
func vdsoCall_zx_process_exit(retcode int64)

//go:noescape
//go:nosplit
func vdsoCall_zx_syscall_next()

//go:noescape
//go:nosplit
func vdsoCall_zx_syscall_test0()

//go:noescape
//go:nosplit
func vdsoCall_zx_syscall_test1()

//go:noescape
//go:nosplit
func vdsoCall_zx_syscall_test2()

//go:noescape
//go:nosplit
func vdsoCall_zx_system_get_page_size() uint32

var (
	vdso_zx_channel_read                   uintptr
	vdso_zx_channel_write                  uintptr
	vdso_zx_clock_get_monotonic            uintptr
	vdso_zx_clock_get_monotonic_via_kernel uintptr
	vdso_zx_handle_close_many              uintptr
	vdso_zx_ktrace_control                 uintptr
	vdso_zx_nanosleep                      uintptr
	vdso_zx_process_exit                   uintptr
	vdso_zx_syscall_next                   uintptr
	vdso_zx_syscall_test0                  uintptr
	vdso_zx_syscall_test1                  uintptr
	vdso_zx_syscall_test2                  uintptr
	vdso_zx_system_get_page_size           uintptr
)
