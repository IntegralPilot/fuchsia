// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::RefCell;
use std::ffi::c_char;
use std::sync::LazyLock;
use zx::sys::zx_handle_t;

mod allocations_table;
mod profiler;
mod recursion_guard;
mod resources_table;
mod waiter_list;

// Do not include the hooks in the tests' executable, to avoid instrumenting the test framework.
#[cfg(not(test))]
mod hooks;
#[cfg(not(test))]
use crate::hooks::enable_quick_early_return;

use crate::profiler::{PerThreadData, Profiler};
use crate::recursion_guard::with_recursion_guard;

// WARNING! Do not change this to use once_cell: once_cell uses parking_lot, which may allocate in
// the contended case.
static PROFILER: LazyLock<Profiler> = LazyLock::new(|| with_recursion_guard(Default::default));

thread_local! {
    static THREAD_DATA: RefCell<PerThreadData> = with_recursion_guard(Default::default);
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct heapdump_global_stats {
    pub total_allocated_bytes: u64,
    pub total_deallocated_bytes: u64,
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct heapdump_thread_local_stats {
    pub total_allocated_bytes: u64,
    pub total_deallocated_bytes: u64,
}

// Calls `f` under a recursion guard, giving it access to the Profiler and the current thread's
// PerThreadData.
pub fn with_profiler(f: impl FnOnce(&Profiler, &mut PerThreadData)) {
    let profiler = &*PROFILER;
    THREAD_DATA.with(|thread_data| {
        with_recursion_guard(|| {
            f(profiler, &mut thread_data.borrow_mut());
        })
    })
}

/// # Safety
/// The caller must pass either a channel handle or an invalid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn heapdump_bind_with_channel(registry_channel: zx_handle_t) {
    let handle = unsafe { zx::NullableHandle::from_raw(registry_channel) };
    if handle.is_invalid() {
        #[cfg(not(test))]
        enable_quick_early_return();

        PROFILER.teardown();
    } else {
        assert_eq!(handle.basic_info().unwrap().object_type, zx::ObjectType::CHANNEL);
        PROFILER.bind(handle.into());
    }
}

/// # Safety
/// The caller must pass suitably-aligned and writable areas of memory to store the stats into.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn heapdump_get_stats(
    global: *mut heapdump_global_stats,
    local: *mut heapdump_thread_local_stats,
) {
    with_profiler(|profiler, thread_data| {
        // Get real data if the profiler is still operating, or zeros if it has been torn down.
        let (global_stats, local_stats) = if let Some(global_stats) = profiler.get_global_stats() {
            (global_stats, thread_data.get_local_stats())
        } else {
            (Default::default(), Default::default())
        };

        if global != std::ptr::null_mut() {
            unsafe {
                *global = global_stats;
            }
        }
        if local != std::ptr::null_mut() {
            unsafe {
                *local = local_stats;
            }
        }
    });
}

/// # Safety
/// The caller must pass a nul-terminated string whose length is not greater than ZX_MAX_NAME_LEN.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn heapdump_take_named_snapshot(name: *const c_char) {
    let name_cstr = unsafe { std::ffi::CStr::from_ptr(name) };
    let name_str = name_cstr.to_str().expect("name contains invalid characters");
    assert!(name_str.len() <= zx::sys::ZX_MAX_NAME_LEN, "name is too long");

    PROFILER.publish_named_snapshot(name_str);
}
