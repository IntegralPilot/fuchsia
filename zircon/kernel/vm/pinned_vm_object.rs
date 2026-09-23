// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::vm::vm_object::VmObject;
use fbl::RefPtr;
use zx_status::Status;

/// An RAII wrapper around a `VmObject` that is pinned.
///
/// Matches the memory layout of the C++ `PinnedVmObject` class.
#[repr(C)]
pub struct PinnedVmObject {
    vmo: Option<RefPtr<VmObject>>,
    offset: usize,
    size: usize,
}

zr::static_assert!(core::mem::size_of::<PinnedVmObject>() == 24);
zr::static_assert!(core::mem::align_of::<PinnedVmObject>() == 8);

impl PinnedVmObject {
    /// Pins `size` bytes starting at `offset` in `vmo` and returns a
    /// `PinnedVmObject`.
    pub fn create(
        vmo: RefPtr<VmObject>,
        offset: u64,
        size: u64,
        write: bool,
    ) -> Result<Self, Status> {
        debug_assert!(page::is_aligned(offset as usize) && page::is_aligned(size as usize));
        vmo.commit_range_pinned(offset, size, write)?;
        Ok(Self { vmo: Some(vmo), offset: offset as usize, size: size as usize })
    }

    /// Returns a reference to the underlying `VmObject`.
    pub fn vmo(&self) -> Option<&RefPtr<VmObject>> {
        self.vmo.as_ref()
    }

    /// Returns the offset into the VMO where the pinned range starts.
    pub fn offset(&self) -> u64 {
        self.offset as u64
    }

    /// Returns the size of the pinned range.
    pub fn size(&self) -> u64 {
        self.size as u64
    }
}

impl Drop for PinnedVmObject {
    fn drop(&mut self) {
        if let Some(vmo) = self.vmo.take() {
            vmo.unpin(self.offset as u64, self.size as u64);
        }
    }
}
