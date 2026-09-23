// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This crate provides Rust bindings for the Stacktrack instrumentation API.

use zx::sys::zx_handle_t;

// From stacktrack/bind.h
unsafe extern "C" {
    fn stacktrack_bind_with_channel(registry_channel: zx_handle_t);
    fn stacktrack_bind_with_fdio();
}

/// Binds the current process to the provided process registry.
///
/// Call either this function or `bind_with_fdio` from the process' main function.
///
/// See also //src/performance/memory/stacktrack/instrumentation/include/stacktrack/bind.h
pub fn bind_with_channel(registry_channel: zx::Channel) {
    // SAFETY: FFI call that takes ownership of the given handle.
    unsafe {
        stacktrack_bind_with_channel(registry_channel.into_raw());
    }
}

/// Binds the current process to the process registry, using `fdio_service_connect` to locate it.
///
/// Call either this function or `bind_with_channel` from the process' main function.
///
/// See also //src/performance/memory/stacktrack/instrumentation/include/stacktrack/bind.h
pub fn bind_with_fdio() {
    // SAFETY: FFI call without arguments.
    unsafe {
        stacktrack_bind_with_fdio();
    }
}
