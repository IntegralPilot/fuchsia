// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::fifo_dispatcher::{FifoDispatcher, FifoDispatcherState};
use super::handle::KernelHandle;
use core::mem::MaybeUninit;
use zx_types::zx_status_t;

unsafe extern "C" {
    pub(crate) fn cpp_fifo_dispatcher_create(
        holder: *mut (),
        count: u32,
        elem_size: u32,
        data: *mut u8,
        handle_out: *mut MaybeUninit<KernelHandle<FifoDispatcher>>,
    ) -> zx_status_t;
}

crate::object::dispatcher::impl_peered_dispatcher_state_init!(
    FifoDispatcher,
    FifoDispatcherState,
    count: u32,
    elem_size: u32,
    data: *mut u8,
);
