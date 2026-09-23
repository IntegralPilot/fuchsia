// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::TpmError;
use crate::ffi_return_if_error;
use std::ptr::null_mut;
use tpm2_tss_sys as tss_sys;

/// Safe wrapper around the `TSS2_TCTI_CONTEXT`. This wrapper ensures that the
/// context is finalized and freed when it is dropped.
pub struct TctiContext {
    tcti_context: *mut tss_sys::TSS2_TCTI_CONTEXT,
}

impl TctiContext {
    /// Attempts to connect to the Fuchsia TCTI transport and return a newly
    /// allocated Fuchsia TCTI from the underlying C Library.
    pub fn try_new() -> Result<Self, TpmError> {
        let mut tcti_context = null_mut();
        ffi_return_if_error!(tss_sys::Tss2_Tcti_Fuchsia_Init_Ex(&mut tcti_context, null_mut()));
        Ok(Self { tcti_context })
    }

    /// Returns a mutable pointer to the underlying mutable C pointer.
    pub(crate) fn as_mut_ptr(&self) -> *mut tss_sys::TSS2_TCTI_CONTEXT {
        self.tcti_context
    }
}

/// Finalizes (which also frees) the underlying tcti_context when the structure
/// goes out of scope.
impl Drop for TctiContext {
    fn drop(&mut self) {
        unsafe { tss_sys::Tss2_Tcti_Fuchsia_Finalize(self.tcti_context) };
    }
}
