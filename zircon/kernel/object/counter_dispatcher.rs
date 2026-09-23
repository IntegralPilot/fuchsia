// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use fbl::Canary;
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_COUNTER_NON_POSITIVE, ZX_COUNTER_POSITIVE, ZX_OBJ_TYPE_COUNTER, ZX_RIGHT_DUPLICATE,
    ZX_RIGHT_INSPECT, ZX_RIGHT_READ, ZX_RIGHT_SIGNAL, ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT,
    ZX_RIGHT_WRITE, zx_rights_t,
};

use super::counter_dispatcher_ffi::cpp_counter_dispatcher_create;

use super::{DispatcherOps, KernelHandle};

use object_constants_rs as object_constants;

// TODO(https://fxbug.dev/532573303): Share this definition with
// zircon/system/public/zircon/rights.h
pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_READ
    | ZX_RIGHT_WRITE
    | ZX_RIGHT_SIGNAL;

// Ensure size and alignment match the constants in object-constants.
zr::static_assert_size_and_align!(
    CounterDispatcherState,
    object_constants::kCounterDispatcherStateSize,
    object_constants::kCounterDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_COUNTER_CREATE_COUNT, "dispatcher.counter.create", Sum);
define_kcounter!(DISPATCHER_COUNTER_DESTROY_COUNT, "dispatcher.counter.destroy", Sum);

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct CounterDispatcherState {
    canary: Canary<{ fbl::magic(b"CNTR") }>,

    #[guarded_by(lock)]
    value: i64,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl CounterDispatcherState {
    pub fn init(
        _dispatcher: *const CounterDispatcher,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_COUNTER_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            value: 0.into(),
            lock <- KMutex::init(),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for CounterDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        DISPATCHER_COUNTER_DESTROY_COUNT.add(1);
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    pub struct CounterDispatcher,
    CounterDispatcherState,
    ZX_OBJ_TYPE_COUNTER,
    object_constants::kCounterDispatcherStateOffset
);

impl CounterDispatcher {
    /// Returns the counter's value.
    pub fn value(&self) -> i64 {
        ksync::lock!(let guard = self.state().lock_lock());
        let fields = guard.fields();
        *fields.value
    }

    /// Sets the counter's value, asserting/deasserting signals as appropriate.
    pub fn set_value(&self, new_val: i64) {
        ksync::lock!(let mut guard = self.state().lock_lock());
        let fields = guard.as_mut().fields_mut();
        let old_val = *fields.value;
        *fields.value = new_val;
        self.update_signals_locked(guard.token(), old_val, new_val);
    }

    /// Adds `amount` to this counter.
    pub fn add(&self, amount: i64) -> Result<(), Status> {
        ksync::lock!(let mut guard = self.state().lock_lock());
        let fields = guard.as_mut().fields_mut();
        let old_val = *fields.value;
        let new_val = match old_val.checked_add(amount) {
            Some(val) => val,
            None => return Err(Status::OUT_OF_RANGE),
        };
        *fields.value = new_val;
        self.update_signals_locked(guard.token(), old_val, new_val);
        Ok(())
    }

    fn update_signals_locked(
        &self,
        token: &ksync::LockToken<'_, CounterDispatcherStateLockClass>,
        old_val: i64,
        new_val: i64,
    ) {
        if old_val <= 0 && new_val > 0 {
            self.update_state_locked(token, ZX_COUNTER_NON_POSITIVE, ZX_COUNTER_POSITIVE);
        } else if old_val > 0 && new_val <= 0 {
            self.update_state_locked(token, ZX_COUNTER_POSITIVE, ZX_COUNTER_NON_POSITIVE);
        }
    }

    /// Creates a new CounterDispatcher via C++ and returns its kernel handle and rights.
    pub fn create() -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        // SAFETY: `cpp_counter_dispatcher_create` initializes the handle on success.
        let handle = unsafe { KernelHandle::create(|out| cpp_counter_dispatcher_create(out)) }?;
        Ok((handle, DEFAULT_RIGHTS))
    }
}
