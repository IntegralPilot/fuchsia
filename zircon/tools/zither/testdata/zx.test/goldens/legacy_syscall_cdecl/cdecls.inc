// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

#ifndef _ZX_SYSCALL_DECL
#error "<zircon/syscalls.h> is the public API header"
#endif

_ZX_SYSCALL_DECL(
    channel_read, zx_status_t, _ZX_SYSCALL_EXTERN_C, 8,
    (handle, options, bytes, handles, num_bytes, num_handles, actual_bytes, actual_handles),
    (_ZX_SYSCALL_ANNO(use_handle("FuchsiaUnchecked")) zx_handle_t handle, uint32_t options,
     void* bytes, _ZX_SYSCALL_ANNO(acquire_handle("FuchsiaUnchecked")) zx_handle_t* handles,
     uint32_t num_bytes, uint32_t num_handles, uint32_t* actual_bytes, uint32_t* actual_handles))

_ZX_SYSCALL_DECL(channel_write, zx_status_t, _ZX_SYSCALL_EXTERN_C, 6,
                 (handle, options, bytes, num_bytes, handles, num_handles),
                 (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t handle, uint32_t options,
                  const void* bytes, uint32_t num_bytes,
                  _ZX_SYSCALL_ANNO(release_handle("Fuchsia")) const zx_handle_t* handles,
                  uint32_t num_handles))

_ZX_SYSCALL_DECL(clock_get_monotonic, zx_time_t, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

_ZX_SYSCALL_DECL(handle_close_many, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2, (handles, num_handles),
                 (_ZX_SYSCALL_ANNO(release_handle("Fuchsia")) const zx_handle_t* handles,
                  size_t num_handles))

_ZX_SYSCALL_DECL(ktrace_control, zx_status_t, _ZX_SYSCALL_EXTERN_C, 4,
                 (handle, action, options, ptr),
                 (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t handle, uint32_t action,
                  uint32_t options, void* ptr))

_ZX_SYSCALL_DECL(nanosleep, zx_status_t, _ZX_SYSCALL_EXTERN_C, 1, (deadline), (zx_time_t deadline))

_ZX_SYSCALL_DECL(process_exit, void, _ZX_SYSCALL_EXTERN_C __NO_RETURN, 1, (retcode),
                 (int64_t retcode))

_ZX_SYSCALL_DECL(system_get_page_size, uint32_t, _ZX_SYSCALL_EXTERN_C __CONST, 0, (), (void))
