// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use fbl::{Canary, HasRefCount};
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_CLOCK_MONOTONIC, ZX_OBJ_TYPE_TIMER, ZX_RIGHT_DUPLICATE, ZX_RIGHT_INSPECT, ZX_RIGHT_SIGNAL,
    ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT, ZX_RIGHT_WRITE, ZX_TIMER_SIGNALED, zx_clock_t, zx_duration_t,
    zx_rights_t, zx_time_t,
};

use crate::kernel::deadline::{Deadline, SlackMode, TimerSlack};
use crate::kernel::timer::Timer;
use crate::platform_rs::timer::{
    DurationUnknown, InstantUnknown, current_boot_time, current_mono_time,
};

use super::timer_dispatcher_ffi::{
    cpp_timer_dispatcher_create, cpp_timer_dispatcher_init_dpc, timer_irq_callback,
};
use super::{DispatcherOps, KernelHandle};

use object_constants_rs as object_constants;

pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_WRITE
    | ZX_RIGHT_SIGNAL;

zr::static_assert_size_and_align!(
    TimerDispatcherState,
    object_constants::kTimerDispatcherStateSize,
    object_constants::kTimerDispatcherStateAlign,
);

zr::define_opaque_storage_ffi! {
    struct DpcStorage(
        object_constants::kDpcStorageSize,
        object_constants::kDpcStorageAlign,
        8, // Matches object_constants::kDpcStorageAlign.
        cpp_timer_dispatcher_init_dpc,
        dispatcher: *const TimerDispatcher,
    );
}

define_kcounter!(DISPATCHER_TIMER_CREATE_COUNT, "dispatcher.timer.create", Sum);
define_kcounter!(DISPATCHER_TIMER_DESTROY_COUNT, "dispatcher.timer.destroy", Sum);

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct TimerDispatcherState {
    canary: Canary<{ fbl::magic(b"TIMR") }>,
    options: u32,
    clock_id: zx_clock_t,

    #[pin]
    dpc: DpcStorage,

    #[guarded_by(lock)]
    deadline: zx_time_t,

    #[guarded_by(lock)]
    slack_amount: zx_duration_t,

    #[guarded_by(lock)]
    cancel_pending: bool,

    #[pin]
    #[guarded_by(lock)]
    timer: Timer,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl TimerDispatcherState {
    pub fn init(
        dispatcher: *const TimerDispatcher,
        options: u32,
        clock_id: zx_clock_t,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_TIMER_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            options,
            clock_id,
            dpc <- unsafe { DpcStorage::init(dispatcher) },
            deadline: 0.into(),
            slack_amount: 0.into(),
            cancel_pending: false.into(),
            timer <- ksync::kcell_init(Timer::init(*clock_id)),
            lock <- KMutex::init(),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for TimerDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        let this = self.project();
        debug_assert_eq!(*this.deadline.get_inner_mut(), 0);
        debug_assert_eq!(*this.slack_amount.get_inner_mut(), 0);
        DISPATCHER_TIMER_DESTROY_COUNT.add(1);
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    pub struct TimerDispatcher,
    TimerDispatcherState,
    ZX_OBJ_TYPE_TIMER,
    object_constants::kTimerDispatcherStateOffset
);

#[must_use = "The return value indicates whether the reference should be released or retained"]
#[derive(Debug, PartialEq, Eq)]
pub enum OnTimerFiredAction {
    /// Release the reference count (the timer callback is done).
    ReleaseRef,
    /// Retain the reference count (the timer was restarted and reused the reference).
    RetainRef,
}

impl TimerDispatcher {
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new TimerDispatcher via C++ and returns its kernel handle and rights.
    pub fn create(
        options: u32,
        clock_id: zx_clock_t,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        if options > zx_types::ZX_TIMER_SLACK_LATE {
            return Err(Status::INVALID_ARGS);
        }
        match options {
            zx_types::ZX_TIMER_SLACK_CENTER
            | zx_types::ZX_TIMER_SLACK_EARLY
            | zx_types::ZX_TIMER_SLACK_LATE => {}
            _ => return Err(Status::INVALID_ARGS),
        }

        // SAFETY: `cpp_timer_dispatcher_create` initializes the handle on success.
        let handle = unsafe {
            KernelHandle::create(|out| cpp_timer_dispatcher_create(options, clock_id, out))
        }?;
        Ok((handle, DEFAULT_RIGHTS))
    }

    pub fn on_zero_handles(&self) {
        // The timers can be kept alive indefinitely by the callbacks, so
        // we need to cancel when there are no more user-mode clients.
        ksync::lock!(let mut guard = self.state().lock_lock());

        if !self.cancel_timer_locked(guard.as_mut()) {
            let fields = guard.as_mut().fields_mut();
            fields.timer.cancel();
        }
    }

    pub fn set(&self, deadline: zx_time_t, slack_amount: zx_duration_t) -> Result<(), Status> {
        self.state().canary.assert();

        ksync::lock!(let mut guard = self.state().lock_lock());

        let did_cancel = self.cancel_timer_locked(guard.as_mut());

        // If the timer is already due, then we can set the signal immediately without
        // starting the timer.
        if deadline == 0 || deadline <= self.current_time() {
            self.update_state_locked(guard.token(), 0, ZX_TIMER_SIGNALED);
            return Ok(());
        }

        let fields = guard.as_mut().fields_mut();
        *fields.deadline = deadline;
        *fields.slack_amount = slack_amount;

        // If we're imminently awaiting a timer callback due to a prior cancellation request,
        // let the callback take care of restarting the timer too so everything happens in the
        // right sequence.
        if *fields.cancel_pending {
            return Ok(());
        }

        // We need to ref-up because the timer and the dpc don't understand
        // refcounted objects. The reference is released either when the RefPtr
        // in rust_timer_dispatcher_on_timer_fired drops or in the complicated
        // cancellation path above.
        fbl::RefPtr::add_ref(self);

        self.set_timer_locked(guard.as_mut(), !did_cancel);

        Ok(())
    }

    pub fn cancel(&self) -> Result<(), Status> {
        self.state().canary.assert();
        ksync::lock!(let mut guard = self.state().lock_lock());
        self.cancel_timer_locked(guard.as_mut());
        Ok(())
    }

    pub fn on_timer_fired(&self) -> OnTimerFiredAction {
        self.state().canary.assert();

        ksync::lock!(let mut guard = self.state().lock_lock());
        let cancel_pending = *guard.as_mut().fields_mut().cancel_pending;

        if cancel_pending {
            // We previously attempted to cancel the timer but the dpc had already
            // been queued. Suppress handling of this callback but take care to
            // restart the timer if its deadline was set in the meantime.
            let fields = guard.as_mut().fields_mut();
            *fields.cancel_pending = false;
            if *fields.deadline != 0 {
                self.set_timer_locked(guard.as_mut(), true);
                OnTimerFiredAction::RetainRef
            } else {
                OnTimerFiredAction::ReleaseRef
            }
        } else {
            // The timer is firing.
            self.update_state_locked(guard.token(), 0, ZX_TIMER_SIGNALED);
            let fields = guard.as_mut().fields_mut();
            *fields.deadline = 0;
            *fields.slack_amount = 0;
            OnTimerFiredAction::ReleaseRef
        }
    }

    pub fn get_info(&self) -> zx_types::zx_info_timer_t {
        self.state().canary.assert();
        ksync::lock!(let guard = self.state().lock_lock());
        let fields = guard.fields();
        zx_types::zx_info_timer_t {
            options: self.state().options,
            clock_id: self.state().clock_id,
            deadline: *fields.deadline,
            slack: *fields.slack_amount,
        }
    }

    fn current_time(&self) -> zx_time_t {
        if self.state().clock_id == ZX_CLOCK_MONOTONIC {
            current_mono_time().0
        } else {
            current_boot_time().0
        }
    }

    fn set_timer_locked(
        &self,
        mut guard: core::pin::Pin<&mut TimerDispatcherStateLockGuard<'_>>,
        cancel_first: bool,
    ) {
        let mut fields = guard.as_mut().fields_mut();

        if cancel_first {
            fields.timer.as_mut().cancel();
        }

        let slack_mode = match self.state().options {
            zx_types::ZX_TIMER_SLACK_CENTER => SlackMode::Center,
            zx_types::ZX_TIMER_SLACK_EARLY => SlackMode::Early,
            zx_types::ZX_TIMER_SLACK_LATE => SlackMode::Late,
            other => panic!("Unknown options: {other:#x}"),
        };

        let slack = TimerSlack::new(DurationUnknown(*fields.slack_amount), slack_mode);
        let slack_deadline = Deadline::new(InstantUnknown(*fields.deadline), slack);

        let dpc_ptr = self.state().dpc.as_void_ptr();

        // SAFETY: `timer_irq_callback` is a valid callback that queues the DPC using `dpc_ptr`.
        // The `TimerDispatcher` (`self`) is guaranteed to remain alive for the duration of the
        // timer callback and DPC execution because we manually incremented its reference count in
        // `set()` using `fbl::RefPtr::add_ref(self)`. That reference is released either when the
        // callback completes or in `cancel_timer_locked()`.
        unsafe {
            fields.timer.set_deadline(&slack_deadline, timer_irq_callback, dpc_ptr);
        }
    }

    fn cancel_timer_locked(
        &self,
        mut guard: core::pin::Pin<&mut TimerDispatcherStateLockGuard<'_>>,
    ) -> bool {
        // Always clear the signal bit.
        self.update_state_locked(guard.token(), ZX_TIMER_SIGNALED, 0);

        let fields = guard.as_mut().fields_mut();

        // If the timer isn't pending then we're done.
        if *fields.deadline == 0 {
            return false; // didn't call timer_cancel
        }
        *fields.deadline = 0;
        *fields.slack_amount = 0;

        // If we're already waiting for the timer to be canceled, then we don't need
        // to cancel it again.
        if *fields.cancel_pending {
            return false; // didn't call timer_cancel
        }

        // The timer is active and needs to be canceled.
        // Refcount is at least 2 because there is a pending timer that we need to cancel.
        let timer_canceled = fields.timer.cancel();
        if timer_canceled {
            // Managed to cancel before OnTimerFired() ran. So we need to decrement the
            // ref count here. The caller must be holding a reference as well, so
            // the refcount must be at least 2 before dropping.
            debug_assert!(self.ref_count().ref_count_debug() >= 2);
            let _ = unsafe { fbl::RefPtr::from_raw(self as *const _) };
        } else {
            // The DPC thread is about to run the callback! Yet we are holding the lock.
            // We'll let the timer callback take care of cleanup.
            *fields.cancel_pending = true;
        }
        true // did call timer_cancel
    }
}
