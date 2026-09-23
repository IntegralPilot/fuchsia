// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::dispatcher::{
    DispatcherOps, PeerHolder, PeerHolderMuClass, PeeredState,
    impl_peered_dispatcher_facade_with_state,
};
use super::event_pair_dispatcher_ffi::cpp_event_pair_dispatcher_create;
use super::handle::KernelHandle;
use crate::counters::define_kcounter;
use core::convert::Infallible;
use core::pin::Pin;
use fbl::{Canary, RefPtr};
use ksync::guarded;
use object_constants_rs::{
    kEventPairDispatcherStateAlign, kEventPairDispatcherStateOffset, kEventPairDispatcherStateSize,
};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_EVENT_SIGNALED, ZX_EVENTPAIR_PEER_CLOSED, ZX_OBJ_TYPE_EVENTPAIR, ZX_RIGHT_DUPLICATE,
    ZX_RIGHT_INSPECT, ZX_RIGHT_SIGNAL, ZX_RIGHT_SIGNAL_PEER, ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT,
    ZX_USER_SIGNAL_ALL, zx_rights_t,
};

pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_SIGNAL
    | ZX_RIGHT_SIGNAL_PEER;

pub const ALLOWED_SIGNALS: u32 = ZX_USER_SIGNAL_ALL | ZX_EVENT_SIGNALED;

zr::static_assert_size_and_align!(
    EventPairDispatcherState,
    kEventPairDispatcherStateSize,
    kEventPairDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_EVENTPAIR_CREATE_COUNT, "dispatcher.eventpair.create", Sum);
define_kcounter!(DISPATCHER_EVENTPAIR_DESTROY_COUNT, "dispatcher.eventpair.destroy", Sum);

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct EventPairDispatcherState {
    canary: Canary<{ fbl::magic(b"EVPD") }>,

    #[pin]
    pub peered: PeeredState<EventPairDispatcher>,
}

impl EventPairDispatcherState {
    pub fn init(holder: RefPtr<PeerHolder<EventPairDispatcher>>) -> impl PinInit<Self, Infallible> {
        DISPATCHER_EVENTPAIR_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            peered <- PeeredState::init(holder),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for EventPairDispatcherState {
    fn drop(self: Pin<&mut Self>) {
        DISPATCHER_EVENTPAIR_DESTROY_COUNT.add(1);
    }
}

impl_peered_dispatcher_facade_with_state!(
    pub struct EventPairDispatcher,
    EventPairDispatcherState,
    ZX_OBJ_TYPE_EVENTPAIR,
    kEventPairDispatcherStateOffset,
    allowed_signals: ALLOWED_SIGNALS,
);

impl EventPairDispatcher {
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new EventPairDispatcher pair and returns their kernel handles and rights.
    pub fn create() -> Result<(KernelHandle<Self>, KernelHandle<Self>, zx_rights_t), Status> {
        let holder0 = PeerHolder::<Self>::create().map_err(|_| Status::NO_MEMORY)?;
        let holder1 = holder0.clone();

        let create_single =
            |holder: RefPtr<PeerHolder<Self>>| -> Result<KernelHandle<Self>, Status> {
                // SAFETY: `holder` transfers an acquired reference count into C++, and
                // `cpp_event_pair_dispatcher_create` initializes `handle` on success.
                unsafe {
                    KernelHandle::create(|out| {
                        cpp_event_pair_dispatcher_create(RefPtr::into_raw(holder) as *mut _, out)
                    })
                }
            };

        let handle0 = create_single(holder0)?;
        let handle1 = create_single(holder1)?;

        handle0.dispatcher().init_peer(handle1.dispatcher().clone());
        handle1.dispatcher().init_peer(handle0.dispatcher().clone());

        Ok((handle0, handle1, DEFAULT_RIGHTS))
    }

    fn on_zero_handles_locked(&self, _token: &ksync::LockToken<'_, PeerHolderMuClass<Self>>) {
        self.state().canary.assert();
    }

    fn on_peer_zero_handles_locked(&self, token: &ksync::LockToken<'_, PeerHolderMuClass<Self>>) {
        self.state().canary.assert();
        self.update_state_locked(token, 0, ZX_EVENTPAIR_PEER_CLOSED);
    }
}
