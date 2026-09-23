// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This crate provides Rust bindings for the Heapdump instrumentation API.

use std::ffi::{CStr, c_char};
use zx::sys::zx_handle_t;

// From heapdump/bind.h
unsafe extern "C" {
    fn heapdump_bind_with_channel(registry_channel: zx_handle_t);
    fn heapdump_bind_with_fdio();
}

// From heapdump/snapshot.h
unsafe extern "C" {
    fn heapdump_take_named_snapshot(snapshot_name: *const c_char);
}

// From heapdump/stats.h
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct GlobalStats {
    pub total_allocated_bytes: u64,
    pub total_deallocated_bytes: u64,
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct ThreadLocalStats {
    pub total_allocated_bytes: u64,
    pub total_deallocated_bytes: u64,
}
unsafe extern "C" {
    fn heapdump_get_stats(global: *mut GlobalStats, local: *mut ThreadLocalStats);
}

/// Binds the current process to the provided process registry.
///
/// Call either this function or `bind_with_fdio` from the process' main function.
///
/// See also //src/performance/memory/heapdump/instrumentation/include/heapdump/bind.h
pub fn bind_with_channel(registry_channel: zx::Channel) {
    // SAFETY: FFI call that takes ownership of the given handle.
    unsafe {
        heapdump_bind_with_channel(registry_channel.into_raw());
    }
}

/// Binds the current process to the process registry, using `fdio_service_connect` to locate it.
///
/// Call either this function or `bind_with_channel` from the process' main function.
///
/// See also //src/performance/memory/heapdump/instrumentation/include/heapdump/bind.h
pub fn bind_with_fdio() {
    // SAFETY: FFI call without arguments.
    unsafe {
        heapdump_bind_with_fdio();
    }
}

/// Publishes a named snapshot of all the current live allocations.
///
/// See also //src/performance/memory/heapdump/instrumentation/include/heapdump/snapshot.h
pub fn take_named_snapshot(snapshot_name: &CStr) {
    // SAFETY: FFI call that takes a NUL-terminated string.
    unsafe {
        heapdump_take_named_snapshot(snapshot_name.as_ptr());
    }
}

/// Obtains stats about past allocations.
///
/// See also //src/performance/memory/heapdump/instrumentation/include/heapdump/stats.h
pub fn get_stats() -> (GlobalStats, ThreadLocalStats) {
    let mut global = GlobalStats::default();
    let mut local = ThreadLocalStats::default();

    // SAFETY: FFI call that writes into the provided structs, that we just allocated on the stack.
    unsafe {
        heapdump_get_stats(&mut global, &mut local);
    }

    (global, local)
}
