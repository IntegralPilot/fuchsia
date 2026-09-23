// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::mem::{align_of, size_of};
use zerocopy::FromBytes;

#[derive(Debug)]
pub enum Error {
    InvalidInput,
}

/// Memory-maps a VMO and mediates access to its memory.
pub struct MemoryMappedVmo {
    map_addr: usize,
    vmo_size: usize,
    writable: bool,
}

impl MemoryMappedVmo {
    /// Maps a VMO in read-only mode.
    ///
    /// Attempting to call methods on the returned object that return mutable references will panic.
    ///
    /// # Safety
    /// The caller must either guarantee that the `vmo` is not modified by others while the returned
    /// instance is alive or that accesses are synchronized in application-specific ways.
    pub unsafe fn new_readonly(vmo: &zx::Vmo) -> Result<MemoryMappedVmo, zx::Status> {
        unsafe { Self::new_impl(vmo, false) }
    }

    /// Maps a VMO in read-write mode.
    ///
    /// # Safety
    /// The caller must either guarantee that the `vmo` is not modified by others while the returned
    /// instance is alive or that accesses are synchronized in application-specific ways.
    pub unsafe fn new_readwrite(vmo: &zx::Vmo) -> Result<MemoryMappedVmo, zx::Status> {
        unsafe { Self::new_impl(vmo, true) }
    }

    /// # Safety
    /// The caller must either guarantee that the `vmo` is not modified by others while the returned
    /// instance is alive or that accesses are synchronized in application-specific ways.
    unsafe fn new_impl(vmo: &zx::Vmo, writable: bool) -> Result<MemoryMappedVmo, zx::Status> {
        let vmo_size = vmo.get_content_size()? as usize;

        let mut flags = zx::VmarFlags::PERM_READ
            | zx::VmarFlags::ALLOW_FAULTS
            | zx::VmarFlags::REQUIRE_NON_RESIZABLE;
        if writable {
            flags |= zx::VmarFlags::PERM_WRITE;
        }

        let map_addr = fuchsia_runtime::vmar_root_self().map(0, &vmo, 0, vmo_size, flags)?;
        Ok(MemoryMappedVmo { map_addr, vmo_size, writable })
    }

    /// Returns the number of usable bytes in the VMO (i.e. its ZX_PROP_VMO_CONTENT_SIZE property,
    /// which is not rounded to the page size).
    pub fn vmo_size(&self) -> usize {
        self.vmo_size
    }

    /// Given an element type, a base offset within the VMO and a number of elements, verifies that
    /// the offset is suitably aligned and that the range of the elements fits in the VMO bounds. If
    /// both conditions are satisfied, return a const pointer to its first element.
    fn validate_and_get_ptr<T>(
        &self,
        byte_offset: usize,
        num_elements: usize,
    ) -> Result<*const T, Error> {
        if byte_offset % align_of::<T>() == 0 {
            if let Some(num_bytes) = size_of::<T>().checked_mul(num_elements) {
                if let Some(end) = byte_offset.checked_add(num_bytes) {
                    if end <= self.vmo_size {
                        return Ok((self.map_addr + byte_offset) as *const T);
                    }
                }
            }
        }

        Err(Error::InvalidInput)
    }

    /// Like validate_and_get_ptr, but returns a mut pointer and panics if the VMO is not writable.
    fn validate_and_get_mut_ptr<T>(
        &mut self,
        byte_offset: usize,
        num_elements: usize,
    ) -> Result<*mut T, Error> {
        if !self.writable {
            panic!("MemoryMappedVmo is not writable");
        }

        Ok(self.validate_and_get_ptr::<T>(byte_offset, num_elements)? as *mut T)
    }

    /// Returns a reference to a slice of elements in the VMO.
    ///
    /// This method validates the alignment and the bounds against the VMO size.
    pub fn get_slice<'a, T: FromBytes>(
        &'a self,
        byte_offset: usize,
        num_elements: usize,
    ) -> Result<&'a [T], Error> {
        let ptr = self.validate_and_get_ptr(byte_offset, num_elements)?;
        unsafe { Ok(std::slice::from_raw_parts(ptr, num_elements)) }
    }

    /// Returns a reference to an element in the VMO.
    ///
    /// This method validates the alignment and the bounds against the VMO size.
    pub fn get_object<'a, T: FromBytes>(&'a self, byte_offset: usize) -> Result<&'a T, Error> {
        let ptr = self.validate_and_get_ptr(byte_offset, 1)?;
        unsafe { Ok(&*ptr) }
    }

    /// Returns a mutable reference to a slice of elements in the VMO.
    ///
    /// This method validates the alignment and the bounds against the VMO size.
    pub fn get_slice_mut<'a, T: FromBytes>(
        &'a mut self,
        byte_offset: usize,
        num_elements: usize,
    ) -> Result<&'a mut [T], Error> {
        let ptr = self.validate_and_get_mut_ptr(byte_offset, num_elements)?;
        unsafe { Ok(std::slice::from_raw_parts_mut(ptr, num_elements)) }
    }

    /// Returns a mutable reference to an element in the VMO.
    ///
    /// This method validates the alignment and the bounds against the VMO size.
    pub fn get_object_mut<'a, T: FromBytes>(
        &'a mut self,
        byte_offset: usize,
    ) -> Result<&'a mut T, Error> {
        let ptr = self.validate_and_get_mut_ptr(byte_offset, 1)?;
        unsafe { Ok(&mut *ptr) }
    }
}

impl Drop for MemoryMappedVmo {
    fn drop(&mut self) {
        // SAFETY: We owned the mapping.
        unsafe {
            fuchsia_runtime::vmar_root_self()
                .unmap(self.map_addr, self.vmo_size)
                .expect("failed to unmap MemoryMappedVmo");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    // Test data used by some of the following tests.
    const TEST_DATA: [u64; 4] = [11, 22, 33, 44];
    const TEST_DATA_SIZE: usize = size_of::<u64>() * TEST_DATA.len();

    #[test]
    fn test_vmo_size() {
        let vmo = zx::Vmo::create(TEST_DATA_SIZE as u64).unwrap();
        let m = unsafe { MemoryMappedVmo::new_readwrite(&vmo) }.unwrap();

        assert_eq!(m.vmo_size(), TEST_DATA_SIZE);
    }

    #[test]
    fn test_write_objects_read_slice() {
        let vmo = zx::Vmo::create(TEST_DATA_SIZE as u64).unwrap();

        // Fill VMO with test data as individual objects.
        {
            let mut m = unsafe { MemoryMappedVmo::new_readwrite(&vmo) }.unwrap();
            for (i, val) in TEST_DATA.iter().enumerate() {
                *m.get_object_mut(size_of::<u64>() * i).unwrap() = *val;
            }
        }

        // Verify that we can read them back correctly as a slice.
        {
            let m = unsafe { MemoryMappedVmo::new_readonly(&vmo) }.unwrap();
            assert_eq!(*m.get_slice::<u64>(0, 4).unwrap(), TEST_DATA);
        }
    }

    #[test]
    fn test_write_slice_read_objects() {
        let vmo = zx::Vmo::create(TEST_DATA_SIZE as u64).unwrap();

        // Fill VMO with test data as a slice.
        {
            let mut m = unsafe { MemoryMappedVmo::new_readwrite(&vmo) }.unwrap();
            m.get_slice_mut(0, 4).unwrap().copy_from_slice(&TEST_DATA);
        }

        // Verify that we can read it back correctly as individual objects.
        {
            let m = unsafe { MemoryMappedVmo::new_readonly(&vmo) }.unwrap();
            for (i, expected_val) in TEST_DATA.iter().enumerate() {
                let actual_val: &u64 = m.get_object(size_of::<u64>() * i).unwrap();
                assert_eq!(*actual_val, *expected_val, "value mismatch at i={}", i);
            }
        }
    }

    #[test]
    fn test_write_slice_read_subslices() {
        const COUNT: usize = 4;
        let vmo = zx::Vmo::create((size_of::<u64>() * COUNT) as u64).unwrap();

        // Fill VMO with test data.
        let mut m = unsafe { MemoryMappedVmo::new_readwrite(&vmo) }.unwrap();
        m.get_slice_mut::<u64>(0, COUNT).unwrap().copy_from_slice(&[11, 22, 33, 44]);

        // Verify that we can read subslices correctly.
        const SECOND_ELEM_BYTE_OFFSET: usize = size_of::<u64>();
        assert_eq!(*m.get_slice::<u64>(SECOND_ELEM_BYTE_OFFSET, 0).unwrap(), []);
        assert_eq!(*m.get_slice::<u64>(SECOND_ELEM_BYTE_OFFSET, 1).unwrap(), [22]);
        assert_eq!(*m.get_slice::<u64>(SECOND_ELEM_BYTE_OFFSET, 2).unwrap(), [22, 33]);
        assert_eq!(*m.get_slice::<u64>(SECOND_ELEM_BYTE_OFFSET, 3).unwrap(), [22, 33, 44]);
    }

    #[test]
    fn test_uninitialized_is_zero() {
        const COUNT: usize = 4;
        let vmo = zx::Vmo::create((size_of::<u64>() * COUNT) as u64).unwrap();
        let m = unsafe { MemoryMappedVmo::new_readonly(&vmo) }.unwrap();

        // Verify that the value of uninitialized data is zero.
        assert_eq!(*m.get_slice::<u64>(0, COUNT).unwrap(), [0; COUNT]);
    }

    #[test]
    fn test_range_errors() {
        const COUNT: usize = 4;
        let vmo = zx::Vmo::create((size_of::<u64>() * COUNT) as u64).unwrap();
        let m = unsafe { MemoryMappedVmo::new_readonly(&vmo) }.unwrap();

        // Reading at a misaligned offset should fail.
        const MISALIGNED_OFFSET: usize = size_of::<u64>() - 1;
        assert_matches!(m.get_object::<u64>(MISALIGNED_OFFSET), Err(Error::InvalidInput));

        // Reading an out-of-bounds range should fail.
        const SECOND_ELEM_BYTE_OFFSET: usize = size_of::<u64>();
        assert_matches!(
            m.get_slice::<u64>(SECOND_ELEM_BYTE_OFFSET, COUNT),
            Err(Error::InvalidInput)
        );
    }

    #[test]
    #[should_panic(expected = "MemoryMappedVmo is not writable")]
    fn test_cannot_get_mutable_slice_from_readonly_vmo() {
        let vmo = zx::Vmo::create(TEST_DATA_SIZE as u64).unwrap();
        let mut m = unsafe { MemoryMappedVmo::new_readonly(&vmo) }.unwrap();

        // This should panic:
        let _ = m.get_slice_mut::<u64>(0, 1);
    }

    #[test]
    #[should_panic(expected = "MemoryMappedVmo is not writable")]
    fn test_cannot_get_mutable_object_from_readonly_vmo() {
        let vmo = zx::Vmo::create(TEST_DATA_SIZE as u64).unwrap();
        let mut m = unsafe { MemoryMappedVmo::new_readonly(&vmo) }.unwrap();

        // This should panic:
        let _ = m.get_object_mut::<u64>(0);
    }
}
