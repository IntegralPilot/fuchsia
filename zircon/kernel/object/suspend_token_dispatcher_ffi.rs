// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::suspend_token_dispatcher::{SuspendTokenDispatcher, SuspendTokenDispatcherState};
use core::mem::MaybeUninit;
use zx_types::zx_status_t;

// C++ FFI declarations
unsafe extern "C" {
    /// Calls into C++ implementation to allocate and construct a SuspendTokenDispatcher.
    ///
    /// # Safety
    ///
    /// `handle_out` must point to writable, uninitialized memory allocated for
    /// `KernelHandle<SuspendTokenDispatcher>`.
    pub fn cpp_suspend_token_dispatcher_create(
        handle_out: *mut MaybeUninit<KernelHandle<SuspendTokenDispatcher>>,
    ) -> zx_status_t;
}

// Trampolines from C++ into Rust SuspendTokenDispatcher / SuspendTokenDispatcherState

crate::object::dispatcher::impl_dispatcher_state_init!(
    SuspendTokenDispatcher,
    SuspendTokenDispatcherState
);

/// Trampoline called when all handles to `dispatcher` have closed.
///
/// # Safety
///
/// `dispatcher` must be a valid reference to an initialized `SuspendTokenDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_suspend_token_dispatcher_on_zero_handles(
    dispatcher: &SuspendTokenDispatcher,
) {
    dispatcher.on_zero_handles();
}
