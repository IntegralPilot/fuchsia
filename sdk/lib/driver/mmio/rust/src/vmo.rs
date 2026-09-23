// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! MMIO regions backed by Fuchsia Virtual Memory Objects.

use crate::memory::Memory;
use crate::region::MmioRegion;
use core::ptr::{NonNull, with_exposed_provenance_mut};
use zx::{CachePolicy, VmarFlags, Vmo};
use zx_status::Status;

/// An active mapping of a Vmo in the root Vmar.
///
/// The mapped memory is kept mapped while the VmoMapping object is alive.
pub struct VmoMapping {
    map_addr: usize,
    map_size: usize,
}

/// A Memory region backed by Vmo mapped memory.
pub type VmoMemory = Memory<VmoMapping>;

impl VmoMapping {
    /// Map the specified memory range of the given Vmo in the root Vmar for this process and
    /// return an object that maintains the mapping for its lifetime.
    ///
    /// Errors:
    /// - [Status::OUT_OF_RANGE]: if `size > isize::MAX`.
    /// - [Status::OUT_OF_RANGE]: if the requested region falls outside of the vmo's memory.
    /// - An error returned by [zx::Vmar::map]: if the mapping fails.
    pub fn map(offset: usize, size: usize, vmo: Vmo) -> Result<MmioRegion<VmoMemory>, Status> {
        Self::map_with_cache_policy(offset, size, vmo, CachePolicy::UnCachedDevice)
    }

    /// Like [`VmoMapping::map`] but specifying a `cache_policy`.
    ///
    /// *NOTE*: Code targeting a real MMIO mapped region must *always* use
    /// [`CachePolicy::UnCachedDevice`] and production code should prefer to use
    /// [`VmoMapping::map`]. This is exposed *exclusively* to allow test code to
    /// pick a different cache policy when `vmo` is not in fact backed by an
    /// actual MMIO VMO.
    pub fn map_with_cache_policy(
        offset: usize,
        size: usize,
        vmo: Vmo,
        cache_policy: CachePolicy,
    ) -> Result<MmioRegion<VmoMemory>, Status> {
        if size > isize::MAX as usize {
            return Err(Status::OUT_OF_RANGE);
        }

        let page_size = zx::system_get_page_size() as usize;
        // Determine how far the offset is into its containing page.
        let page_offset = offset % page_size;

        // Round the offset down to a page boundary.
        let offset = (offset - page_offset) as u64;

        // Round the mapped size up so it covers complete pages.
        let map_size = (size + page_offset).next_multiple_of(page_size);

        let info = vmo.info()?;
        if info.cache_policy() != cache_policy {
            vmo.set_cache_policy(cache_policy)?;
        }
        if offset.saturating_add(map_size as u64) > info.size_bytes {
            return Err(Status::OUT_OF_RANGE);
        }

        let root_self = fuchsia_runtime::vmar_root_self();
        let map_addr = root_self.map(
            0,
            &vmo,
            offset,
            map_size,
            VmarFlags::PERM_READ | VmarFlags::PERM_WRITE | VmarFlags::MAP_RANGE,
        )?;
        let base_ptr =
            NonNull::<u8>::new(with_exposed_provenance_mut(map_addr + page_offset)).unwrap();
        let len = size;

        let mapping = Self { map_addr, map_size };

        // Safety:
        // - the range from base_ptr to base_ptr + len is within the range exclusively owned by the
        // mapping.
        // - the mapping is used as the claim - this keeps the memory valid for the lifetime of the
        // claim.
        let memory = unsafe { Memory::new_unchecked(mapping, base_ptr, len) };
        Ok(MmioRegion::new(memory))
    }
}

/// Unmaps the memory.
impl Drop for VmoMapping {
    fn drop(&mut self) {
        let root_self = fuchsia_runtime::vmar_root_self();
        // Safety:
        // - This object only exposes the mapped memory range through the `memory_range` function
        // whose safety requirements require the caller to only use this memory while the
        // VmoMapping is alive.
        let _ = unsafe { root_self.unmap(self.map_addr, self.map_size) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mmio;
    use zx::Rights;

    #[test]
    fn test_mapping() {
        const TEST_LEN: usize = 256;
        let vmo = Vmo::create(TEST_LEN as u64).unwrap();

        let mut mmio = VmoMapping::map(0, TEST_LEN, vmo).unwrap().into_split_send();

        for i in 0..TEST_LEN {
            assert_eq!(mmio.try_store8(i, i as u8), Ok(()));
        }

        for i in 0..TEST_LEN {
            assert_eq!(mmio.try_load8(i), Ok(i as u8));
        }
    }

    #[test]
    fn test_page_offset() {
        const VMO_SIZE: u64 = 1024;
        let vmo = Vmo::create(VMO_SIZE).unwrap();

        // Write the offset of every 16 bit location into that location.
        for i in (0..VMO_SIZE).step_by(2) {
            let addr = i as u16;
            vmo.write(&addr.to_le_bytes(), i).unwrap();
        }

        const TEST_OFFSET: usize = 128;
        const TEST_LEN: usize = 256;
        let mmio = VmoMapping::map(TEST_OFFSET, TEST_LEN, vmo).unwrap();

        for i in (0..TEST_LEN).step_by(2) {
            assert_eq!(mmio.try_load16(i), Ok((i + TEST_OFFSET) as u16));
        }
    }

    #[test]
    fn test_mapping_unmaps() {
        const TEST_LEN: usize = 256;
        let vmo = Vmo::create(TEST_LEN as u64).unwrap();
        let vmo_read_handle: Vmo = vmo.duplicate_handle(Rights::READ).unwrap();

        {
            let _mapping = VmoMapping::map(0, TEST_LEN, vmo).unwrap();
            // The vmo should be mapped exactly once.
            assert_eq!(vmo_read_handle.info().unwrap().num_mappings, 1);
        }

        // The mapping should have been unmapped by now.
        assert_eq!(vmo_read_handle.info().unwrap().num_mappings, 0);
    }
}
