// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

KERNEL_SYSCALL(
    channel_read, zx_status_t, _ZX_SYSCALL_EXTERN_C, 8,
    (handle, options, bytes, handles, num_bytes, num_handles, actual_bytes, actual_handles),
    (_ZX_SYSCALL_ANNO(use_handle("FuchsiaUnchecked")) zx_handle_t handle, uint32_t options,
     user_out_ptr<void> bytes,
     _ZX_SYSCALL_ANNO(acquire_handle("FuchsiaUnchecked")) user_out_ptr<zx_handle_t> handles,
     uint32_t num_bytes, uint32_t num_handles, user_out_ptr<uint32_t> actual_bytes,
     user_out_ptr<uint32_t> actual_handles))

KERNEL_SYSCALL(channel_write, zx_status_t, _ZX_SYSCALL_EXTERN_C, 6,
               (handle, options, bytes, num_bytes, handles, num_handles),
               (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t handle, uint32_t options,
                user_in_ptr<const void> bytes, uint32_t num_bytes,
                _ZX_SYSCALL_ANNO(release_handle("Fuchsia")) user_in_ptr<const zx_handle_t> handles,
                uint32_t num_handles))

VDSO_SYSCALL(clock_get_monotonic, zx_time_t, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

INTERNAL_SYSCALL(clock_get_monotonic_via_kernel, zx_time_t, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

KERNEL_SYSCALL(handle_close_many, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2, (handles, num_handles),
               (_ZX_SYSCALL_ANNO(release_handle("Fuchsia")) user_in_ptr<const zx_handle_t> handles,
                size_t num_handles))

KERNEL_SYSCALL(ktrace_control, zx_status_t, _ZX_SYSCALL_EXTERN_C, 4, (handle, action, options, ptr),
               (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t handle, uint32_t action,
                uint32_t options, user_inout_ptr<void> ptr))

BLOCKING_SYSCALL(nanosleep, zx_status_t, _ZX_SYSCALL_EXTERN_C, 1, (deadline), (zx_time_t deadline))

KERNEL_SYSCALL(process_exit, void, _ZX_SYSCALL_EXTERN_C __NO_RETURN, 1, (retcode),
               (int64_t retcode))

KERNEL_SYSCALL(syscall_next, void, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

KERNEL_SYSCALL(syscall_test0, void, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

KERNEL_SYSCALL(syscall_test1, void, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

KERNEL_SYSCALL(syscall_test2, void, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

VDSO_SYSCALL(system_get_page_size, uint32_t, _ZX_SYSCALL_EXTERN_C __CONST, 0, (), (void))
