// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ffi::c_void;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};
use fbl::{Canary, RefPtr};
use zr::ToMutPtr;
use zx_status::Status;
use zx_types::zx_signals_t;

use crate::kernel::event::Event;

use super::dispatcher::Dispatcher;
use super::{HandleRef, HandleTableReadGuard};
use object_constants_rs as object_constants;

unsafe extern "C" {
    fn cpp_wait_signal_observer_init(observer: *mut c_void);
    fn cpp_wait_signal_observer_destroy(observer: *mut c_void);
}

zr::static_assert_size_and_align!(
    WaitSignalObserverState,
    object_constants::kWaitSignalObserverStorageSize,
    object_constants::kWaitSignalObserverStorageAlign,
);

/// The Rust state for `WaitSignalObserver`, stored in the C++ object's `opaque_storage_`.
#[repr(C, align(8))]
pub struct WaitSignalObserverState {
    canary: Canary<{ fbl::magic(b"WTSO") }>,
    event: *const Event,
    dispatcher: Option<RefPtr<Dispatcher>>,
    final_signal_state: AtomicU32,
}

impl WaitSignalObserverState {
    /// Callback when watched signals match.
    pub fn on_match(&self, signals: zx_signals_t, queue_to_own: *mut c_void) {
        self.canary.assert();
        // Save the signal state, and wake our waiter.
        self.final_signal_state.store(signals, Ordering::Release);
        // SAFETY: `self.event` is a valid Event pointer set during begin().
        unsafe {
            (*self.event).signal_etc(Ok(()), queue_to_own);
        }
    }

    /// Callback when handle is cancelled or closed.
    pub fn on_cancel(&self, signals: zx_signals_t) {
        self.canary.assert();
        // Save the signal state, and wake our waiter.
        self.final_signal_state
            .store(signals | zx_types::ZX_SIGNAL_HANDLE_CLOSED, Ordering::Release);
        // SAFETY: `self.event` is a valid Event pointer set during begin().
        unsafe {
            (*self.event).signal_etc(Status::CANCELED, core::ptr::null_mut());
        }
    }
}

impl Drop for WaitSignalObserverState {
    fn drop(&mut self) {
        self.canary.assert();
        debug_assert!(self.dispatcher.is_none());
    }
}

/// FFI initialization of `WaitSignalObserverState` called from C++ constructor.
///
/// # Safety
///
/// `storage` must point to valid uninitialized memory of size and alignment
/// matching `WaitSignalObserverState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_wait_signal_observer_init(
    storage: *mut MaybeUninit<WaitSignalObserverState>,
) {
    // SAFETY: `storage` is non-null and points to uninitialized storage of matching size and align.
    unsafe {
        (*storage).write(WaitSignalObserverState {
            canary: Canary::new(),
            event: core::ptr::null(),
            dispatcher: None,
            final_signal_state: AtomicU32::new(0),
        });
    }
}

/// FFI destruction of `WaitSignalObserverState` called from C++ destructor.
///
/// # Safety
///
/// `storage` must point to a valid initialized `WaitSignalObserverState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_wait_signal_observer_destroy(storage: *mut WaitSignalObserverState) {
    // SAFETY: `storage` points to an initialized `WaitSignalObserverState`.
    unsafe {
        core::ptr::drop_in_place(storage);
    }
}

/// FFI callback when watched signals match.
///
/// # Safety
///
/// `state` must point to a valid initialized `WaitSignalObserverState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_wait_signal_observer_on_match(
    state: &WaitSignalObserverState,
    signals: zx_signals_t,
    queue_to_own: *mut c_void,
) {
    state.on_match(signals, queue_to_own);
}

/// FFI callback when handle is cancelled or closed.
///
/// # Safety
///
/// `state` must point to a valid initialized `WaitSignalObserverState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_wait_signal_observer_on_cancel(
    state: &WaitSignalObserverState,
    signals: zx_signals_t,
) {
    state.on_cancel(signals);
}

/// Helper struct for waiting on `object_wait_one` and `object_wait_many` syscalls.
///
/// Wraps the underlying C++ `WaitSignalObserver` which participates in the `SignalObserver`
/// intrusive list on `Dispatcher`.
#[repr(C, align(8))]
pub struct WaitSignalObserver {
    storage: zr::OpaqueBytes<{ object_constants::kWaitSignalObserverSize }>,
}

zr::static_assert!(
    core::mem::size_of::<WaitSignalObserver>() == object_constants::kWaitSignalObserverSize
);
zr::static_assert!(
    core::mem::align_of::<WaitSignalObserver>() == object_constants::kWaitSignalObserverAlign
);

impl Default for WaitSignalObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitSignalObserver {
    /// Creates a new `WaitSignalObserver` initialized in-place.
    pub fn new() -> Self {
        let mut observer = Self { storage: zr::OpaqueBytes::default() };
        // SAFETY: `observer` has `kWaitSignalObserverSize` bytes aligned to `kWaitSignalObserverAlign`,
        // matching `WaitSignalObserver` layout.
        unsafe {
            cpp_wait_signal_observer_init(observer.as_mut_ptr());
        }
        observer
    }

    #[inline]
    fn as_mut_ptr(&mut self) -> *mut c_void {
        self.storage.to_mut_ptr().cast()
    }

    #[inline]
    fn as_ptr(&self) -> *const c_void {
        self.storage.to_mut_ptr().cast::<c_void>() as *const c_void
    }

    #[inline]
    fn state(&self) -> &WaitSignalObserverState {
        // SAFETY: `self.as_ptr()` points to an initialized C++ `WaitSignalObserver` whose
        // `opaque_storage_` is located at offset `kWaitSignalObserverStorageOffset`.
        unsafe {
            let state_ptr =
                self.as_ptr().cast::<u8>().add(object_constants::kWaitSignalObserverStorageOffset);
            &*state_ptr.cast::<WaitSignalObserverState>()
        }
    }

    #[inline]
    fn state_mut(&mut self) -> &mut WaitSignalObserverState {
        // SAFETY: `self.as_mut_ptr()` points to an initialized C++ `WaitSignalObserver` whose
        // `opaque_storage_` is located at offset `kWaitSignalObserverStorageOffset`.
        unsafe {
            let state_ptr = self
                .as_mut_ptr()
                .cast::<u8>()
                .add(object_constants::kWaitSignalObserverStorageOffset);
            &mut *state_ptr.cast::<WaitSignalObserverState>()
        }
    }

    #[inline]
    fn signal_observer_ptr(&mut self) -> *mut c_void {
        self.as_mut_ptr()
    }

    /// Begins observing signals on the given handle while holding the handle table lock.
    ///
    /// # Invariants
    ///
    /// Calling this requires a `HandleTableReadGuard` to prove that the handle table lock is held.
    /// If this succeeds, `end()` must be called before the `Event` is destroyed.
    pub fn begin(
        &mut self,
        _guard: &HandleTableReadGuard<'_>,
        event: &Event,
        handle: &HandleRef<'_>,
        watched_signals: zx_signals_t,
    ) -> Result<(), Status> {
        let signal_observer_ptr = self.signal_observer_ptr();
        let state = self.state_mut();
        state.canary.assert();
        debug_assert!(state.dispatcher.is_none());

        state.event = event as *const Event;
        let dispatcher = handle.dispatcher();

        // Wait for one of `watched_signals` to become active.
        //
        // Note that `watched_signals` may be 0, in which case we won't receive
        // a callback, but will remain queued until `end` is called.
        // SAFETY: `signal_observer_ptr` and `handle.as_ptr()` point to valid initialized objects.
        unsafe { dispatcher.add_observer(signal_observer_ptr, handle.as_ptr(), watched_signals) }?;

        state.dispatcher = Some(dispatcher);
        Ok(())
    }

    /// Ends observing signals and returns the final observed signals.
    ///
    /// This should *not* be called under the handle table lock.
    pub fn end(&mut self) -> zx_signals_t {
        let signal_observer_ptr = self.signal_observer_ptr();
        let state = self.state_mut();
        state.canary.assert();
        debug_assert!(state.dispatcher.is_some());

        let dispatcher = state.dispatcher.take().expect("dispatcher must be present");
        let mut signals: zx_signals_t = 0;
        // SAFETY: `signal_observer_ptr` points to a valid observer.
        let was_removed = unsafe { dispatcher.remove_observer(signal_observer_ptr, &mut signals) };

        // If `was_removed` is false, it means a callback was fired and `final_signal_state` has a value.
        if !was_removed {
            return state.final_signal_state.load(Ordering::Acquire);
        }

        // Otherwise, return the set of signals at the point of removal.
        signals
    }
}

impl Drop for WaitSignalObserver {
    fn drop(&mut self) {
        // SAFETY: `self` is a valid `WaitSignalObserver`.
        unsafe {
            cpp_wait_signal_observer_destroy(self.as_mut_ptr());
        }
    }
}
