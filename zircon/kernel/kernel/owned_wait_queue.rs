// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::convert::Infallible;
use core::ffi::c_void;
use object_constants_rs as object_constants;
use pin_init::{PinInit, pin_data};

unsafe extern "C" {
    fn cpp_owned_wait_queue_init(wait_queue: *mut c_void);
    fn cpp_owned_wait_queue_destroy(wait_queue: *mut c_void);
    fn cpp_owned_wait_queue_reset_owner_if_no_waiters(wait_queue: *mut c_void);
}

/// Pinned FFI wrapper around C++ `OwnedWaitQueue`.
#[pin_data(PinnedDrop)]
#[repr(C, align(8))]
pub struct OwnedWaitQueue {
    _opaque: zr::OpaqueBytes<{ object_constants::kOwnedWaitQueueSize }>,
}

// SAFETY: `OwnedWaitQueue` is thread-safe and internally synchronized via chainlocks.
unsafe impl Send for OwnedWaitQueue {}
unsafe impl Sync for OwnedWaitQueue {}

zr::static_assert_size_and_align!(
    OwnedWaitQueue,
    object_constants::kOwnedWaitQueueSize,
    object_constants::kOwnedWaitQueueAlign,
);

impl OwnedWaitQueue {
    /// In-place pinned initializer for `OwnedWaitQueue`.
    pub fn init() -> impl PinInit<Self, Infallible> {
        zr::pin_init_ffi!(cpp_owned_wait_queue_init)
    }

    /// Returns a raw mutable pointer to the underlying C++ `OwnedWaitQueue`.
    pub fn as_ptr(&self) -> *mut c_void {
        self._opaque.get().cast()
    }

    /// Resets the queue owner if there are no waiters currently queued.
    pub fn reset_owner_if_no_waiters(&self) {
        // SAFETY: `self` is a valid pointer to an initialized OwnedWaitQueue.
        unsafe {
            cpp_owned_wait_queue_reset_owner_if_no_waiters(self.as_ptr());
        }
    }
}

zr::unsafe_pinned_drop_ffi!(OwnedWaitQueue, cpp_owned_wait_queue_destroy);
