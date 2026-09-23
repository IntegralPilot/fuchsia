// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::counter_dispatcher::{CounterDispatcher, CounterDispatcherState};
use super::handle::KernelHandle;
use core::mem::MaybeUninit;
use zx_types::zx_status_t;

// C++ FFI declarations
unsafe extern "C" {
    pub(crate) fn cpp_counter_dispatcher_create(
        handle_out: *mut MaybeUninit<KernelHandle<CounterDispatcher>>,
    ) -> zx_status_t;
}

// FFI trampolines for C++ calling into Rust CounterDispatcherState

crate::object::dispatcher::impl_dispatcher_state_init!(CounterDispatcher, CounterDispatcherState);
