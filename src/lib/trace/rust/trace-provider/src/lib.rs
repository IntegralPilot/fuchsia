// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ffi::CStr;
use core::ptr::{NonNull, null};
use libasync::{AsAsyncDispatcherRef, AsyncDispatcher};
use zx::sys::zx_handle_t;

/// Creates a trace provider service that enables traces created by a process
/// to be collected by the system trace manager.
///
/// Typically applications would call this method once, early in their main
/// function to enable them to be eligible to produce traces.
///
/// It is safe but unnecessary to call this function more than once.
pub fn trace_provider_create_with_fdio() {
    unsafe {
        sys::trace_provider_create_with_fdio_rust();
    }
}

pub fn trace_provider_create_with_service(to_service_h: zx_handle_t) {
    unsafe {
        sys::trace_provider_create_with_service_rust(to_service_h);
    }
}

/// Wait for trace provider initialization to acknowledge already-running traces before returning.
///
/// If the current thread is expected to initialize the provider then this should only be called
/// after doing so to avoid a deadlock.
pub fn trace_provider_wait_for_init() {
    unsafe {
        sys::trace_provider_wait_for_init();
    }
}

/// Use this object to start and run a trace provider under an existing [`libasync`] dispatcher
/// handle.
///
/// You can use this in combination with `libasync_scope_dispatcher` to run the trace
/// provider on top of fuchsia-async, for example:
///
/// ```no_run
/// #[fuchsia::main]
/// async fn main() {
///     // create a new scoped async dispatcher object for the current fuchsia-async executor.
///     let scope_dispatcher = libasync_scope_dispatcher::ScopeDispatcher::new();
///     // create a trace provider on it.
///     let trace_provider =
///         fuchsia_trace_provider::TraceProvider::new_with_fdio(&scope_dispatcher, None);
///
///     // run your program here...
///
///     // drop the trace provider before shutting down the dispatcher since it will queue work
///     // on the dispatcher.
///     drop(trace_provider);
///     // shutdown the dispatcher.
///     scope_dispatcher.shutdown().await;
/// }
/// ```
///
/// Note: As in the example above, make sure that this object is dropped before the dispatcher
/// shuts down.
pub struct TraceProvider {
    trace_provider: NonNull<core::ffi::c_void>,
    // just kept here to ensure that the dispatcher object is kept alive for memory safety reasons.
    #[expect(unused)]
    dispatcher: AsyncDispatcher,
}

impl TraceProvider {
    pub fn new_with_fdio(
        dispatcher: &impl AsAsyncDispatcherRef,
        name: Option<&CStr>,
    ) -> Option<Self> {
        let dispatcher = AsyncDispatcher::new(dispatcher);
        let trace_provider = NonNull::new(unsafe {
            sys::trace_provider_create_with_fdio(
                dispatcher.as_ptr().as_ptr(),
                name.map_or(null(), CStr::as_ptr),
            )
        })?;
        Some(Self { trace_provider, dispatcher })
    }
}

impl Drop for TraceProvider {
    fn drop(&mut self) {
        unsafe { sys::trace_provider_destroy(self.trace_provider.as_ptr()) }
    }
}

mod sys {
    // From librust-trace-provider.so
    unsafe extern "C" {
        // See the C++ documentation for these functions in trace_provider.cc
        pub(super) fn trace_provider_create_with_fdio_rust();
        pub(super) fn trace_provider_create_with_service_rust(to_service_h: zx::sys::zx_handle_t);
        pub(super) fn trace_provider_wait_for_init();

        // These are directly imported from the C++ trace library.
        pub(super) fn trace_provider_create_with_fdio(
            dispatcher: *const libasync_sys::async_dispatcher_t,
            name: *const core::ffi::c_char,
        ) -> *mut core::ffi::c_void;
        pub(super) fn trace_provider_destroy(trace_provider: *const core::ffi::c_void);
    }
}
