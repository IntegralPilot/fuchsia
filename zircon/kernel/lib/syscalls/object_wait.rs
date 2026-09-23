// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::deadline::Deadline;
use crate::kernel::event::Event;
use crate::kernel::thread::Interruptible;
use crate::object::{
    AutoBlocked, Blocked, HandleTableReadGuard, HandleValue, ProcessDispatcher, WaitSignalObserver,
};
use crate::platform_rs::timer::InstantUnknown;
use crate::user_copy::{UserInOutPtr, UserOutPtr};
use debug::ltracef;
use fbl::InlineArray;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_RIGHT_WAIT, ZX_SIGNAL_HANDLE_CLOSED, ZX_WAIT_MANY_MAX_ITEMS, zx_instant_mono_t,
    zx_signals_t, zx_wait_item_t,
};

const LOCAL_TRACE: u32 = 0;

zr::static_assert!(ZX_WAIT_MANY_MAX_ITEMS <= zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize);

#[syscall]
pub fn sys_object_wait_one(
    handle_value: HandleValue,
    signals: zx_signals_t,
    deadline: zx_instant_mono_t,
    observed: UserOutPtr<zx_signals_t>,
) -> Result<(), Status> {
    ltracef!("handle {:?}\n", handle_value);

    pin_init::stack_pin_init!(let event = Event::init_unsignaled());
    let mut wait_signal_observer = WaitSignalObserver::new();

    let slack_deadline = ProcessDispatcher::with_current(|up| {
        pin_init::stack_pin_init!(let guard = HandleTableReadGuard::new(up));

        let handle = guard.get_handle(handle_value).ok_or(Status::BAD_HANDLE)?;
        if !handle.has_rights(ZX_RIGHT_WAIT) {
            return Err(Status::ACCESS_DENIED);
        }

        wait_signal_observer.begin(&guard, event.as_ref().get_ref(), &handle, signals)?;

        let slack = up.get_timer_slack_policy();
        Ok(Deadline::new(InstantUnknown(deadline), slack))
    })?;

    // Event::Wait() will return ZX_OK if already signaled,
    // even if the deadline has passed. It will return ZX_ERR_TIMED_OUT
    // after the deadline passes if the event has not been
    // signaled.
    let wait_result = {
        let _blocked = AutoBlocked::new(Blocked::WAIT_ONE);
        event.wait(&slack_deadline)
    };

    // Regardless of wait outcome, we must call End().
    let signals_state = wait_signal_observer.end();

    if !observed.is_null() {
        observed.write(signals_state)?;
    }

    if (signals_state & ZX_SIGNAL_HANDLE_CLOSED) != 0 {
        return Err(Status::CANCELED);
    }

    wait_result
}

#[syscall]
pub fn sys_object_wait_many(
    user_items: UserInOutPtr<zx_wait_item_t>,
    count: usize,
    deadline: zx_instant_mono_t,
) -> Result<(), Status> {
    ltracef!("count {}\n", count);

    let slack = ProcessDispatcher::with_current(|up| up.get_timer_slack_policy());
    let slack_deadline = Deadline::new(InstantUnknown(deadline), slack);

    if count == 0 {
        let now = crate::platform_rs::timer::current_mono_time();
        {
            let _blocked = AutoBlocked::new(Blocked::WAIT_MANY);
            crate::kernel::thread::sleep_etc(&slack_deadline, Interruptible::YES, now.0)?;
        }
        return Err(Status::TIMED_OUT);
    }

    if count > ZX_WAIT_MANY_MAX_ITEMS {
        return Err(Status::OUT_OF_RANGE);
    }

    let mut uninit_items =
        [const { core::mem::MaybeUninit::<zx_wait_item_t>::uninit() }; ZX_WAIT_MANY_MAX_ITEMS];
    let items = user_items.copy_slice_from_user(&mut uninit_items[..count])?;

    const MAX_INLINE_OBSERVERS: usize = 8;

    // WaitSignalObserver is heavier than it looks so make sure we know how
    // much stack InlineArray is going to use, given limited kernel stack size.
    zr::static_assert!(core::mem::size_of::<WaitSignalObserver>() * MAX_INLINE_OBSERVERS < 640);

    let mut observers = InlineArray::<WaitSignalObserver, MAX_INLINE_OBSERVERS>::try_new(count)
        .map_err(|_| Status::NO_MEMORY)?;

    pin_init::stack_pin_init!(let event = Event::init_unsignaled());

    // We may need to unwind (which can be done outside the lock).
    let mut num_added = 0;
    let begin_result = ProcessDispatcher::with_current(|up| {
        pin_init::stack_pin_init!(let guard = HandleTableReadGuard::new(up));

        for (ix, item) in items[..count].iter().enumerate() {
            let handle =
                guard.get_handle(HandleValue::new(item.handle)).ok_or(Status::BAD_HANDLE)?;
            if !handle.has_rights(ZX_RIGHT_WAIT) {
                return Err(Status::ACCESS_DENIED);
            }

            observers[ix].begin(&guard, event.as_ref().get_ref(), &handle, item.waitfor)?;
            num_added += 1;
        }
        Ok(())
    });

    if let Err(err) = begin_result {
        for obs in &mut observers[..num_added] {
            obs.end();
        }
        return Err(err);
    }

    // Event::Wait() will return ZX_OK if already signaled,
    // even if deadline has passed. It will return ZX_ERR_TIMED_OUT
    // after the deadline passes if the event has not been
    // signaled.
    let wait_result = {
        let _blocked = AutoBlocked::new(Blocked::WAIT_MANY);
        event.wait(&slack_deadline)
    };

    // Regardless of wait outcome, we must call End().
    let mut combined = 0;
    for (ix, obs) in observers.iter_mut().enumerate() {
        let pending = obs.end();
        items[ix].pending = pending;
        combined |= pending;
    }

    user_items.copy_slice_to_user(items)?;

    if (combined & ZX_SIGNAL_HANDLE_CLOSED) != 0 {
        return Err(Status::CANCELED);
    }

    wait_result
}
