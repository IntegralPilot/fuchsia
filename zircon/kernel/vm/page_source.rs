// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::pin::Pin;
use page_source_bindings as bindings;
use pin_init::pin_data;
use zr::{Opaque, pin_init_ffi, unsafe_pinned_drop_ffi};

/// Wrapper around tracking multiple different page requests that might need waiting. Only one
/// individual request is allowed to be considered 'active' at a time as the one that next needs
/// waiting on. Tracking whether a request is active is, depending on the request type, partially
/// automatic and partially requiring additional input from the user.
/// The PageRequest and LazyPageRequest access methods do not currently have a way to enforce that
/// those specific types of requests are made with the returned objects, however this could change
/// and callers are expected to use the correct method.
/// TODO(adanis): Implement an enforcement strategy.
#[pin_data(PinnedDrop)]
pub struct MultiPageRequest {
    /// The optional inner `PageRequest` can appear in intrusive containers, so this object must be
    /// pinned.
    #[pin]
    opaque: Opaque<bindings::MultiPageRequest>,
}

unsafe_pinned_drop_ffi!(MultiPageRequest, bindings::cpp_multi_page_request_destroy);

impl MultiPageRequest {
    /// Returns an in-place initializer for stack-pinning a `MultiPageRequest`.
    pub fn new() -> impl pin_init::PinInit<Self> {
        /// # Safety
        /// `ptr` must point to uninitialized `MultiPageRequest` storage.
        unsafe fn init_shim(ptr: *mut core::ffi::c_void) {
            let req_ptr: *mut bindings::MultiPageRequest = ptr.cast();
            // SAFETY: `ptr` is guaranteed by `pin_init_ffi!` to point to valid `MultiPageRequest`
            // storage.
            unsafe { bindings::cpp_multi_page_request_construct(req_ptr) }
        }
        pin_init_ffi!(init_shim)
    }

    /// Cancel all requests and have no active request.
    pub fn cancel_requests(self: Pin<&mut Self>) {
        // SAFETY: Calling C++ CancelRequests on the pinned instance does not move it.
        unsafe { bindings::cpp_multi_page_request_cancel_requests(self.as_raw()) }
    }

    /// Returns a raw pointer to the underlying C++ `MultiPageRequest`.
    ///
    /// Callers must not use the returned raw pointer to move the object in memory.
    pub fn as_raw(self: Pin<&mut Self>) -> *mut bindings::MultiPageRequest {
        // SAFETY: Obtaining a raw pointer to `opaque` does not move the pinned object.
        unsafe { self.get_unchecked_mut().opaque.get() }
    }
}
