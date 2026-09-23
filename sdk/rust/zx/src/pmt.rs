// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon pmt objects.

use crate::{NullableHandle, Status, ok, sys};
use std::mem;

/// An object representing a Zircon Pinned Memory Token.
/// See [PMT Documentation](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/pinned_memory_token) for details.
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Pmt(NullableHandle);
impl_handle_based!(Pmt);

impl Pmt {
    /// Unpins a Pinned Memory Token.
    /// Wraps the
    /// [`zx_pmt_unpin`](https://fuchsia.dev/fuchsia-src/reference/syscalls/pmt_unpin) system call
    /// to unpin a pmt.
    ///
    /// This function is marked `unsafe` because calling it at the wrong time (i.e., while hardware
    /// is still accessing the pinned memory) can lead to memory corruption.
    ///
    /// # Safety
    ///
    /// The caller must ensure hardware is no longer accessing the memory that has been pinned or
    /// else risk arbitrary memory corruption. It is strongly recommended you do not call this in
    /// a drop method.
    pub unsafe fn unpin(self) -> Result<(), Status> {
        let status = unsafe { sys::zx_pmt_unpin(self.raw_handle()) };

        // According to the syscall documentation, after calling `zx_pmt_unpin`,
        // the pmt handle cannot be used anymore, not even for `zx_handle_close`.
        // This is also true even if the function returns an error.
        mem::forget(self);

        ok(status)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vmo;

    #[test]
    fn pmt_unpin_wrong_handle() {
        let vmo = Vmo::create(0).unwrap();
        let pmt = unsafe { Pmt::from(NullableHandle::from_raw(vmo.into_raw())) };

        let status = unsafe { pmt.unpin() };
        assert_eq!(status, Err(Status::WRONG_TYPE));
    }
}
