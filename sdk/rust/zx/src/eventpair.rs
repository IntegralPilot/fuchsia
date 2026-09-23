// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon event pairs.

use crate::{NullableHandle, Peered, ok};

/// An object representing a Zircon
/// [event_pair](https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts#events_event_pairs)
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct EventPair(NullableHandle);
impl_handle_based!(EventPair);
impl Peered for EventPair {}

impl EventPair {
    /// Create an event pair, a pair of objects which can signal each other. Wraps the
    /// [zx_eventpair_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/eventpair_create.md)
    /// syscall.
    ///
    /// # Panics
    ///
    /// If the kernel reports no available memory to create an event pair or the process' job
    /// policy disallows EventPair creation.
    pub fn create() -> (Self, Self) {
        let mut out0 = 0;
        let mut out1 = 0;
        let options = 0;
        let status = unsafe { crate::sys::zx_eventpair_create(options, &mut out0, &mut out1) };
        ok(status).expect(
            "eventpair creation always succeeds except with OOM or when job policy denies it",
        );
        unsafe {
            (Self::from(NullableHandle::from_raw(out0)), Self::from(NullableHandle::from_raw(out1)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Duration, MonotonicInstant, Signals, WaitResult};

    #[test]
    fn wait_and_signal_peer() {
        let (p1, p2) = EventPair::create();
        let eighty_ms = Duration::from_millis(80);

        // Waiting on one without setting any signal should time out.
        assert_eq!(
            p2.wait_one(Signals::USER_0, MonotonicInstant::after(eighty_ms)),
            WaitResult::TimedOut(Signals::empty())
        );

        // If we set a signal, we should be able to wait for it.
        assert!(p1.signal_peer(Signals::NONE, Signals::USER_0).is_ok());
        assert_eq!(
            p2.wait_one(Signals::USER_0, MonotonicInstant::after(eighty_ms)).unwrap(),
            Signals::USER_0
        );

        // Should still work, signals aren't automatically cleared.
        assert_eq!(
            p2.wait_one(Signals::USER_0, MonotonicInstant::after(eighty_ms)).unwrap(),
            Signals::USER_0
        );

        // Now clear it, and waiting should time out again.
        assert!(p1.signal_peer(Signals::USER_0, Signals::NONE).is_ok());
        assert_eq!(
            p2.wait_one(Signals::USER_0, MonotonicInstant::after(eighty_ms)),
            WaitResult::TimedOut(Signals::empty())
        );
    }
}
