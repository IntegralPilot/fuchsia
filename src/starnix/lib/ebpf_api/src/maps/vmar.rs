// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::Deref;

/// A RAII container for a vmar. The vmar mapping will be destroyed when the container is dropped.
/// User must ensure not to use the vmar mapping afterwards.
#[derive(Debug)]
pub struct AllocatedVmar {
    vmar: zx::Vmar,
    base: usize,
}

impl AllocatedVmar {
    /// Allocate a new subregion in `parent`. Returns an `AllocatedVmar` that keep the region alive
    /// and the base address of the subregion.
    ///
    /// # Safety
    ///
    /// The returned object will deallocate the region when dropped. It is the responsibility of
    /// the caller to ensure the returned memory is only used as long as this object is alive.
    pub unsafe fn allocate(
        parent: &zx::Vmar,
        offset: usize,
        size: usize,
        flags: zx::VmarFlags,
    ) -> Result<Self, zx::Status> {
        parent.allocate(offset, size, flags).map(|(vmar, base)| Self { vmar, base })
    }

    pub fn base(&self) -> usize {
        self.base
    }
}

impl Drop for AllocatedVmar {
    fn drop(&mut self) {
        // SAFETY:
        //
        // This call is safe only if the user of the `AllocatedVmar` ensured the memory is not used
        // anymore when the object is dropped. This is the contract of the `AllocatedVmar::new`
        // method that is itself unsafe.
        unsafe {
            // Ignore errors. Thete is nothing to do at this point.
            let _ = self.vmar.destroy();
        }
    }
}

impl Deref for AllocatedVmar {
    type Target = zx::Vmar;

    fn deref(&self) -> &Self::Target {
        &self.vmar
    }
}
