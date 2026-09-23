// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::marker::{PhantomData, PhantomPinned};
use pin_init::pin_data;
use zr::OpaqueBytes;

unsafe extern "C" {
    fn cpp_timer_init(timer: *mut core::ffi::c_void, clock_id: u32);
    fn cpp_timer_destroy(timer: *mut Timer);
    fn cpp_timer_set(
        timer: *mut Timer,
        deadline: *const Deadline,
        callback: Callback,
        arg: *mut core::ffi::c_void,
    );
    fn cpp_timer_cancel(timer: *mut Timer) -> bool;
}

pub const ZX_CLOCK_MONOTONIC: u32 = 0;
pub const ZX_CLOCK_BOOT: u32 = 1;

use super::deadline::Deadline;
use crate::platform_rs::timer::InstantUnknown;

pub type Callback = unsafe extern "C" fn(timer: *mut Timer, now: i64, arg: *mut core::ffi::c_void);

const TIMER_SIZE: usize = 72;

#[pin_data(PinnedDrop)]
#[repr(C)]
#[repr(align(8))]
pub struct Timer {
    _opaque: OpaqueBytes<TIMER_SIZE>,
    _marker: PhantomData<PhantomPinned>,
}

impl Timer {
    pub fn init(clock_id: u32) -> impl pin_init::PinInit<Self, core::convert::Infallible> {
        zr::pin_init_ffi!(cpp_timer_init, clock_id)
    }

    /// Schedules the timer to fire at the specified deadline.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - `callback` is a valid FFI callback that does not cause undefined behavior when executed.
    /// - `arg` is either a valid pointer or a null pointer, and is safe to pass to `callback`.
    pub unsafe fn set_deadline(
        self: core::pin::Pin<&mut Self>,
        deadline: &Deadline,
        callback: Callback,
        arg: *mut core::ffi::c_void,
    ) {
        // SAFETY: cpp_timer_set is safe as self and deadline are valid pointers.
        unsafe {
            let me = self.get_unchecked_mut();
            cpp_timer_set(me as *mut Timer, deadline as *const Deadline, callback, arg);
        }
    }

    /// Schedules the timer to fire once at the specified deadline (in nanoseconds).
    ///
    /// # Safety
    ///
    /// Same as [`Self::set_deadline`].
    pub unsafe fn set_oneshot(
        mut self: core::pin::Pin<&mut Self>,
        deadline: i64,
        callback: Callback,
        arg: *mut core::ffi::c_void,
    ) {
        let dl = Deadline::no_slack(InstantUnknown(deadline));
        // SAFETY: The caller guarantees the safety of callback and arg.
        unsafe {
            self.as_mut().set_deadline(&dl, callback, arg);
        }
    }

    pub fn cancel(self: core::pin::Pin<&mut Self>) -> bool {
        // SAFETY: cpp_timer_cancel is safe as self is a valid pointer.
        unsafe {
            let me = self.get_unchecked_mut();
            cpp_timer_cancel(me as *mut Timer)
        }
    }
}

zr::unsafe_pinned_drop_ffi!(Timer, cpp_timer_destroy);

// SAFETY: A Timer can be sent across threads as its underlying C++ object is thread-safe
// (protected by TimerLock internally).
unsafe impl Send for Timer {}

zr::static_assert!(core::mem::size_of::<Timer>() == 72);
zr::static_assert!(core::mem::align_of::<Timer>() == 8);

/// Kernel timer tests.
#[cfg(ktest)]
#[unittest::suite(name = "timer_rust")]
mod tests {
    use super::{Timer, ZX_CLOCK_MONOTONIC};
    use crate::platform_rs::timer::DurationMono;
    use core::sync::atomic::{AtomicBool, Ordering};
    use pin_init::stack_pin_init;

    unsafe extern "C" {
        fn cpp_current_mono_time() -> i64;
    }

    unsafe extern "C" fn timer_cb(_timer: *mut Timer, _now: i64, arg: *mut core::ffi::c_void) {
        // SAFETY: arg is a valid pointer to AtomicBool.
        let fired = unsafe { &*(arg as *const AtomicBool) };
        fired.store(true, Ordering::Release);
    }

    /// Verifies that a scheduled oneshot timer fires and executes its callback.
    #[test]
    fn test_timer_oneshot() {
        stack_pin_init!(let timer = Timer::init(ZX_CLOCK_MONOTONIC));
        let fired = AtomicBool::new(false);
        let arg = &fired as *const AtomicBool as *mut core::ffi::c_void;

        // Schedule timer to fire in 1ms (1,000,000 ns) from now.
        let now = unsafe { cpp_current_mono_time() };
        // SAFETY: timer_cb and arg are valid.
        unsafe {
            timer.as_mut().set_oneshot(now + 1_000_000, timer_cb, arg);
        }

        // Poll up to 100ms (100 iterations of 1ms sleep) for the timer to fire.
        let mut success = false;
        for _ in 0..100 {
            if fired.load(Ordering::Acquire) {
                success = true;
                break;
            }
            let _ = crate::kernel::thread::sleep_relative(DurationMono(1_000_000)); // 1ms
        }

        unittest::expect_true!(success);
    }

    /// Verifies that a scheduled timer can be successfully cancelled.
    #[test]
    fn test_timer_cancel() {
        stack_pin_init!(let timer = Timer::init(ZX_CLOCK_MONOTONIC));
        let fired = AtomicBool::new(false);
        let arg = &fired as *const AtomicBool as *mut core::ffi::c_void;

        // Schedule timer to fire in 10 seconds (in the future).
        let now = unsafe { cpp_current_mono_time() };
        // SAFETY: timer_cb and arg are valid.
        unsafe {
            timer.as_mut().set_oneshot(now + 10_000_000_000, timer_cb, arg);
        }

        // Cancel it immediately.
        let cancelled = timer.as_mut().cancel();
        // Cancel should return true.
        unittest::expect_true!(cancelled);

        // Sleep for a short duration to ensure no callback is running.
        let _ = crate::kernel::thread::sleep_relative(DurationMono(1_000_000)); // 1ms

        // Verify that it did not fire.
        unittest::expect_false!(fired.load(Ordering::Acquire));
    }
}
