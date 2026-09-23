// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

_ZX_SYSCALL_DECL(clock_get_monotonic, void, /* no attributes */, 1, (time),
                 (zither_syscalls_time_t * time))

_ZX_SYSCALL_DECL(handle_close_many, zither_syscalls_status_t, /* no attributes */, 2,
                 (handles, num_handles),
                 (_ZX_SYSCALL_ANNO(release_handle("Fuchsia"))
                      const zither_syscalls_handle_t* handles,
                  size_t num_handles))

_ZX_SYSCALL_DECL(nanosleep, zither_syscalls_status_t, /* no attributes */, 1, (deadline),
                 (zither_syscalls_time_t deadline))

_ZX_SYSCALL_DECL(process_exit, void, __NO_RETURN, 1, (retcode), (int64_t retcode))

_ZX_SYSCALL_DECL(system_get_page_size, void, __CONST, 1, (size), (uint32_t * size))
