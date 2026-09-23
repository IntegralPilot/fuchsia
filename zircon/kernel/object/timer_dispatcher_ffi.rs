// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::timer_dispatcher::{OnTimerFiredAction, TimerDispatcher, TimerDispatcherState};
use core::mem::MaybeUninit;
use zx_types::{zx_clock_t, zx_status_t};

// C++ FFI declarations
unsafe extern "C" {
    pub(crate) fn cpp_timer_dispatcher_create(
        options: u32,
        clock_id: zx_clock_t,
        handle_out: *mut MaybeUninit<KernelHandle<TimerDispatcher>>,
    ) -> zx_status_t;

    pub(crate) fn cpp_timer_dispatcher_init_dpc(
        dpc_storage: *mut core::ffi::c_void,
        disp: *const TimerDispatcher,
    );

    pub(crate) fn timer_irq_callback(
        timer: *mut crate::kernel::timer::Timer,
        now: i64,
        arg: *mut core::ffi::c_void,
    );
}

// Trampolines from C++ into Rust TimerDispatcher / TimerDispatcherState

/// # Safety
///
/// `ptr` must point to uninitialized memory of at least `size_of::<TimerDispatcherState>()`
/// bytes with proper alignment, and `dispatcher` points to the enclosing C++ `TimerDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_timer_dispatcher_state_init(
    ptr: *mut TimerDispatcherState,
    dispatcher: *const TimerDispatcher,
    options: u32,
    clock_id: zx_clock_t,
) {
    unsafe {
        let _ = pin_init::PinInit::__pinned_init(
            TimerDispatcherState::init(dispatcher, options, clock_id),
            ptr,
        );
    }
}

/// # Safety
///
/// `disp` must be a valid reference to an initialized `TimerDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_timer_dispatcher_on_zero_handles(disp: &TimerDispatcher) {
    disp.on_zero_handles();
}

/// # Safety
///
/// `disp` must be a valid pointer to an initialized `TimerDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_timer_dispatcher_on_timer_fired(disp: *const TimerDispatcher) {
    let disp = unsafe { fbl::RefPtr::from_raw(disp) };
    match disp.on_timer_fired() {
        OnTimerFiredAction::ReleaseRef => {}
        OnTimerFiredAction::RetainRef => core::mem::forget(disp),
    }
}

/// # Safety
///
/// `disp` must be a valid reference to an initialized `TimerDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_timer_dispatcher_get_info(
    disp: &TimerDispatcher,
) -> zx_types::zx_info_timer_t {
    disp.get_info()
}
