// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::event_pair_dispatcher::{EventPairDispatcher, EventPairDispatcherState};
use super::handle::KernelHandle;
use core::mem::MaybeUninit;
use zx_types::{ZX_OK, zx_rights_t, zx_status_t};

unsafe extern "C" {
    pub(crate) fn cpp_event_pair_dispatcher_create(
        holder: *mut (),
        handle_out: *mut MaybeUninit<KernelHandle<EventPairDispatcher>>,
    ) -> zx_status_t;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_event_pair_dispatcher_create(
    handle0_out: *mut MaybeUninit<KernelHandle<EventPairDispatcher>>,
    handle1_out: *mut MaybeUninit<KernelHandle<EventPairDispatcher>>,
    rights_out: *mut zx_rights_t,
) -> zx_status_t {
    unsafe {
        match EventPairDispatcher::create() {
            Ok((handle0, handle1, rights)) => {
                (*handle0_out).write(handle0);
                (*handle1_out).write(handle1);
                rights_out.write(rights);
                ZX_OK
            }
            Err(status) => status.into_raw(),
        }
    }
}

crate::object::dispatcher::impl_peered_dispatcher_state_init!(
    EventPairDispatcher,
    EventPairDispatcherState,
);
