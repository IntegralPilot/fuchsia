// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::KernelHandle;
use super::event_dispatcher_ffi::{
    cpp_event_dispatcher_create, cpp_event_dispatcher_get_mem_pressure_event,
    cpp_memory_stall_event_dispatcher_create,
};
use crate::counters::define_kcounter;
use core::mem::MaybeUninit;
use fbl::Canary;
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_OBJ_TYPE_EVENT, ZX_RIGHT_DUPLICATE, ZX_RIGHT_INSPECT, ZX_RIGHT_SIGNAL, ZX_RIGHT_TRANSFER,
    ZX_RIGHT_WAIT, zx_duration_mono_t, zx_rights_t,
};

use object_constants_rs as object_constants;

pub const DEFAULT_RIGHTS: zx_rights_t =
    ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE | ZX_RIGHT_WAIT | ZX_RIGHT_INSPECT | ZX_RIGHT_SIGNAL;

zr::static_assert_size_and_align!(
    EventDispatcherState,
    object_constants::kEventDispatcherStateSize,
    object_constants::kEventDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_EVENT_CREATE_COUNT, "dispatcher.event.create", Sum);
define_kcounter!(DISPATCHER_EVENT_DESTROY_COUNT, "dispatcher.event.destroy", Sum);

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct EventDispatcherState {
    canary: Canary<{ fbl::magic(b"EVTD") }>,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl EventDispatcherState {
    pub fn init(
        _dispatcher: *const EventDispatcher,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_EVENT_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            lock <- KMutex::init(),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for EventDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        DISPATCHER_EVENT_DESTROY_COUNT.add(1);
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    pub struct EventDispatcher,
    EventDispatcherState,
    ZX_OBJ_TYPE_EVENT,
    object_constants::kEventDispatcherStateOffset
);

impl EventDispatcher {
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new EventDispatcher via C++ and returns its kernel handle and rights.
    pub fn create(options: u32) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        // SAFETY: `cpp_event_dispatcher_create` initializes `handle` on success.
        let handle =
            unsafe { KernelHandle::create(|out| cpp_event_dispatcher_create(options, out)) }?;
        Ok((handle, DEFAULT_RIGHTS))
    }

    /// Returns the kernel-owned memory pressure event dispatcher for the given kind.
    pub fn get_mem_pressure_event(kind: u32) -> fbl::RefPtr<EventDispatcher> {
        let mut out = MaybeUninit::<fbl::RefPtr<EventDispatcher>>::zeroed();
        // SAFETY: `out` points to valid uninitialized memory for a `RefPtr`.
        unsafe {
            cpp_event_dispatcher_get_mem_pressure_event(kind, out.as_mut_ptr());
            out.assume_init()
        }
    }

    /// Creates a memory stall watch event dispatcher.
    pub fn create_memory_stall(
        kind: u32,
        threshold: zx_duration_mono_t,
        window: zx_duration_mono_t,
    ) -> Result<(KernelHandle<EventDispatcher>, zx_rights_t), Status> {
        // SAFETY: `cpp_memory_stall_event_dispatcher_create` initializes `handle` and `rights`
        // on success.
        unsafe {
            KernelHandle::create_with_rights(|h_out, r_out| {
                cpp_memory_stall_event_dispatcher_create(kind, threshold, window, h_out, r_out)
            })
        }
    }
}
