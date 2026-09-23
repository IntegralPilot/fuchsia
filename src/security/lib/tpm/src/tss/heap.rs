// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::c_void;
use std::ptr::null_mut;
use tpm2_tss_sys as tss_sys;

/// A convenience macro to handle the function call pattern that is common
/// to the entire Enhanced System API which returns an error on failure. We
/// almost always propagate this error and this simple macro both handles the
/// correct invocation of the unsafe body and returns immediately on failure.
#[macro_export]
macro_rules! ffi_return_if_error {
    ($body:expr) => {{
        let return_code: tss_sys::TSS2_RC = unsafe { $body };
        if return_code != 0 {
            return Err(TpmError::TssReturnCode(return_code));
        }
    }};
}

/// The `EsysHeapObj` is a generic type that wraps any `tpm2_tss_sys`
/// pointer that can be freed using the `Esys_Free` function. This prevents
/// accidental memory leaks in the low level Rust layer that calls through to
/// the underlying unsafe bindings.
pub struct EsysHeapObj<T> {
    pub inner: *mut T,
}

impl<T> EsysHeapObj<T> {
    /// Creates a pointer for the provided type and assigns it to
    /// null. The assumption is the inner memory will be allocated by
    /// the C library in an internal function call.
    pub fn null() -> Self {
        Self { inner: null_mut() }
    }
}

/// Returns a reference to the inner type, handling the unsafe
/// dereferencing. If the pointer is null this method will panic.
impl<T> AsRef<T> for EsysHeapObj<T> {
    fn as_ref(&self) -> &T {
        if self.inner.is_null() {
            panic!("EsysHeapObj attempted to dereference a null pointer.");
        }
        unsafe { &*self.inner }
    }
}

/// Returns a reference to the inner type, handling the unsafe
/// dereferencing. If the pointer is null this method will panic.
impl<T> AsMut<T> for EsysHeapObj<T> {
    fn as_mut(&mut self) -> &mut T {
        if self.inner.is_null() {
            panic!("EsysHeapObj attempted to dereference a null pointer.");
        }
        unsafe { &mut *self.inner }
    }
}

/// Drop casts the inner pointer to a `ffi::c_void` and frees the underlying
/// memory using `Esys_Free`.
impl<T> Drop for EsysHeapObj<T> {
    fn drop(&mut self) {
        unsafe { tss_sys::Esys_Free(self.inner as *mut c_void) }
    }
}
