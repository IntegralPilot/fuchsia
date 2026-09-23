// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Accessors for the `zx::Clock` representing UTC, used by common operations.
//! Standard library functions for accessing UTC or "wall clock time" all work
//! by reading this clock.

use zx::sys::{ZX_HANDLE_INVALID, zx_handle_t, zx_status_t};
use zx::{BootTimeline, Clock, NullableHandle, Status, SyntheticTimeline, Timeline, Unowned};

// <zircon/utc.h>
unsafe extern "C" {
    fn _zx_utc_reference_get() -> zx_handle_t;

    fn _zx_utc_reference_swap(
        new_utc_reference: zx_handle_t,
        prev_utc_reference_out: *mut zx_handle_t,
    ) -> zx_status_t;
}

/// The `zx_libc` crate does not provide the `zx::Timeline` type for UTC, but
/// the functions here are generic over a supplied `zx::Timeline` type.
pub type UtcClock<T = SyntheticTimeline> = Clock<BootTimeline, T>;

/// Returns a handle to the currently assigned UTC clock, or a null handle if
/// no such clock currently exists.
///
/// Thread safety is the responsibility of the user.  In particular, if a clock
/// is fetched by a user using utc::reference_get, but then the clock is
/// swapped out using utc::reference_swap and the original clock is closed,
/// then the initial clock handle returned is now invalid and could result in a
/// use-after-close situation.  It is the user's responsibility to avoid these
/// situations.
pub fn reference_get<T: Timeline>() -> Unowned<'static, UtcClock<T>> {
    // SAFETY: basic FFI call.
    unsafe { Unowned::from_raw_handle(_zx_utc_reference_get()) }
}

/// Atomically swap the clock handle provided with the current UTC reference.
pub fn reference_swap<T: Timeline>(new_clock: UtcClock<T>) -> Result<UtcClock<T>, Status> {
    // SAFETY: basic FFI call.
    Ok(unsafe {
        let mut old = ZX_HANDLE_INVALID;
        Status::ok(_zx_utc_reference_swap(new_clock.into_raw(), &mut old))?;
        NullableHandle::from_raw(old)
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zx::ClockOpts;

    fn fake_clock() -> UtcClock {
        UtcClock::create(ClockOpts::MONOTONIC, None).expect("cannot create test clock")
    }

    #[test]
    fn test_get() {
        let test_clock = fake_clock();
        let original_clock = reference_get();
        assert_ne!(original_clock, test_clock.unowned())
    }

    #[test]
    fn test_swap() {
        let original_clock = reference_get();
        let new_clock = fake_clock();

        // SAFETY: Hidden borrow used only for comparison below.
        let test_clock = unsafe { Unowned::<UtcClock>::from_raw_handle(new_clock.raw_handle()) };

        let real_clock = reference_swap(new_clock).expect("cannot swap");
        assert_eq!(real_clock.unowned(), original_clock);

        let got_clock = reference_get();

        // Put the real clock back before assertions could fail.
        let swapped_clock = reference_swap(real_clock).expect("cannot swap back");

        assert_ne!(original_clock, test_clock);
        assert_eq!(got_clock, test_clock);
        assert_eq!(swapped_clock.unowned(), test_clock);
    }
}
