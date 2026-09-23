// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sys::zx_wait_item_t;
use crate::{AsHandleRef, HandleRef, MonotonicInstant, Signals, Status, sys};

/// A "wait item" containing a handle reference and information about what signals
/// to wait on, and, on return from `object_wait_many`, which are pending.
///
/// Returned from `wait_item()` methods on handle newtypes.
///
/// ABI-compatible with `zx_wait_item_t`.
#[repr(C)]
#[derive(Debug)]
pub struct WaitItem<'a> {
    /// The handle to wait on.
    handle: HandleRef<'a>,
    /// A set of signals to wait for.
    waiting_for: Signals,
    /// The set of signals pending, on return of `object_wait_many`.
    pending: Signals,
}

// Assert that WaitItem is ABI-equivalent to zx_wait_item_t. We don't use the crate's
// static_assert_align!() macro here because it's hard to get that macro to work with lifetimes.
static_assertions::assert_eq_size!(WaitItem<'_>, zx_wait_item_t);
static_assertions::assert_eq_align!(WaitItem<'_>, zx_wait_item_t);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(WaitItem<'_>, handle),
    std::mem::offset_of!(zx_wait_item_t, handle),
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(WaitItem<'_>, waiting_for),
    std::mem::offset_of!(zx_wait_item_t, waitfor),
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(WaitItem<'_>, pending),
    std::mem::offset_of!(zx_wait_item_t, pending),
);

impl<'a> WaitItem<'a> {
    /// Creates a new `WaitItem` for the given handle and signals.
    pub(crate) fn new(handle: HandleRef<'a>, waiting_for: Signals) -> Self {
        // SAFETY: call requires the same invariants that NullableHandleRef has for its handles.
        let handle = unsafe { HandleRef::from_raw_handle(handle.raw_handle()) };
        Self { handle, waiting_for, pending: Signals::empty() }
    }

    /// Returns a reference to the contained handle, if any.
    pub fn handle(&self) -> HandleRef<'a> {
        self.handle.clone()
    }

    /// Returns the signals that this WaitItem is asing the kernel to wait for.
    pub fn waiting_for(&self) -> Signals {
        self.waiting_for
    }

    /// Returns the set of signals pending as written by `object_wait_many`.
    pub fn pending(&self) -> Signals {
        self.pending
    }
}

impl<'a> AsHandleRef for WaitItem<'a> {
    fn as_handle_ref(&self) -> HandleRef<'a> {
        self.handle()
    }
}

/// Wait on multiple handles.
/// The success return value is a bool indicating whether one or more of the
/// provided handle references was closed during the wait.
///
/// Wraps the
/// [zx_object_wait_many](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_wait_many.md)
/// syscall.
pub fn object_wait_many(
    items: &mut [WaitItem<'_>],
    deadline: MonotonicInstant,
) -> Result<bool, Status> {
    // SAFETY: WaitItem is ABI-compatible with zx_wait_item_t. The pointer is valid for the kernel
    // to write to for the slice's whole length because we're guaranteed exclusivity by &mut.
    let status = unsafe {
        sys::zx_object_wait_many(
            items.as_mut_ptr().cast::<zx_wait_item_t>(),
            items.len(),
            deadline.into_nanos(),
        )
    };
    if status == sys::ZX_ERR_CANCELED {
        return Ok(true);
    }
    Status::ok(status).map(|()| false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Duration, Event, WaitResult};

    #[test]
    fn wait_and_signal() {
        let event = Event::create();
        let ten_ms = Duration::from_millis(10);

        // Waiting on it without setting any signal should time out.
        assert_eq!(
            event.wait_one(Signals::USER_0, MonotonicInstant::after(ten_ms)),
            WaitResult::TimedOut(Signals::empty())
        );

        // If we set a signal, we should be able to wait for it.
        assert!(event.signal(Signals::NONE, Signals::USER_0).is_ok());
        assert_eq!(
            event.wait_one(Signals::USER_0, MonotonicInstant::after(ten_ms)).unwrap(),
            Signals::USER_0
        );

        // Should still work, signals aren't automatically cleared.
        assert_eq!(
            event.wait_one(Signals::USER_0, MonotonicInstant::after(ten_ms)).unwrap(),
            Signals::USER_0
        );

        // Now clear it, and waiting should time out again.
        assert!(event.signal(Signals::USER_0, Signals::NONE).is_ok());
        assert_eq!(
            event.wait_one(Signals::USER_0, MonotonicInstant::after(ten_ms)),
            WaitResult::TimedOut(Signals::empty())
        );
    }

    #[test]
    fn wait_many_and_signal() {
        let ten_ms = Duration::from_millis(10);
        let e1 = Event::create();
        let e2 = Event::create();

        // Waiting on them now should time out.
        let mut items = [e1.wait_item(Signals::USER_0), e2.wait_item(Signals::USER_1)];
        assert_eq!(
            object_wait_many(&mut items[..], MonotonicInstant::after(ten_ms)),
            Err(Status::TIMED_OUT)
        );
        assert_eq!(items[0].pending(), Signals::NONE);
        assert_eq!(items[1].pending(), Signals::NONE);

        // Signal one object and it should return success.
        assert!(e1.signal(Signals::NONE, Signals::USER_0).is_ok());
        assert!(object_wait_many(&mut items, MonotonicInstant::after(ten_ms)).is_ok());
        assert_eq!(items[0].pending(), Signals::USER_0);
        assert_eq!(items[1].pending(), Signals::NONE);

        // Signal the other and it should return both.
        assert!(e2.signal(Signals::NONE, Signals::USER_1).is_ok());
        assert!(object_wait_many(&mut items, MonotonicInstant::after(ten_ms)).is_ok());
        assert_eq!(items[0].pending(), Signals::USER_0);
        assert_eq!(items[1].pending(), Signals::USER_1);

        // Clear signals on both; now it should time out again.
        assert!(e1.signal(Signals::USER_0, Signals::NONE).is_ok());
        assert!(e2.signal(Signals::USER_1, Signals::NONE).is_ok());
        assert_eq!(
            object_wait_many(&mut items, MonotonicInstant::after(ten_ms)),
            Err(Status::TIMED_OUT)
        );
        assert_eq!(items[0].pending(), Signals::NONE);
        assert_eq!(items[1].pending(), Signals::NONE);
    }
}
