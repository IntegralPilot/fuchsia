// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

// Re-export the types defined in the zx-types crate
pub use zx_types::*;

// Only link against zircon when targeting Fuchsia
#[cfg(target_os = "fuchsia")]
#[link(name = "zircon")]
unsafe extern "C" {

    pub fn zx_channel_read(
        handle: zx_handle_t,
        options: u32,
        bytes: *mut u8,
        handles: *mut zx_handle_t,
        num_bytes: u32,
        num_handles: u32,
        actual_bytes: *mut u32,
        actual_handles: *mut u32,
    ) -> zx_status_t;

    pub fn zx_channel_write(
        handle: zx_handle_t,
        options: u32,
        bytes: *const u8,
        num_bytes: u32,
        handles: *const zx_handle_t,
        num_handles: u32,
    ) -> zx_status_t;

    pub fn zx_clock_get_monotonic() -> zx_time_t;

    pub fn zx_handle_close_many(handles: *const zx_handle_t, num_handles: usize) -> zx_status_t;

    pub fn zx_ktrace_control(
        handle: zx_handle_t,
        action: u32,
        options: u32,
        ptr: *mut u8,
    ) -> zx_status_t;

    pub fn zx_nanosleep(deadline: zx_time_t) -> zx_status_t;

    pub fn zx_process_exit(retcode: i64);

    pub fn zx_syscall_next();

    pub fn zx_syscall_test0();

    pub fn zx_syscall_test1();

    pub fn zx_syscall_test2();

    pub fn zx_system_get_page_size() -> u32;

}
