// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod atomic_vec;
pub use atomic_vec::AtomicBitVec;

use memory_mapped_vmo::MemoryMappedVmo;
use std::sync::atomic::{AtomicU64, Ordering};
use zx::Rights;

pub struct PageRefaultCounter {
    vmo: zx::Vmo,
    _storage: MemoryMappedVmo,
    count_ptr: *const AtomicU64,
}

// SAFETY: both `vmo`` and `_storage`` are Send, and they are not modified once created (thus, they
// are Sync). `count_ptr` is a pointer, pointing to the memory mapped region managed by
// `MemoryMappedVmo`. It is valid as long as `_storage` is valid, and the pointer is not
// invalidated by moving `PageRefaultCounter` as the memory mapped region stays in the same place.
unsafe impl Send for PageRefaultCounter {}
unsafe impl Sync for PageRefaultCounter {}

impl PageRefaultCounter {
    /// Creates a new read-write PageRefaultCounter.
    pub fn new() -> Result<Self, zx::Status> {
        let vmo = zx::Vmo::create(size_of::<AtomicU64>().try_into().unwrap())?;
        vmo.set_name(&zx::Name::new_lossy("page_refault_counter"))?;

        // SAFETY: all accesses to [storage] are synchronized (through an Atomic).
        let mut storage: MemoryMappedVmo = unsafe { MemoryMappedVmo::new_readwrite(&vmo)? };
        let count_ptr: *mut AtomicU64 =
            storage.get_object_mut::<AtomicU64>(0).map_err(|_| zx::Status::INVALID_ARGS)?;
        Ok(PageRefaultCounter { vmo: vmo, _storage: storage, count_ptr })
    }

    /// Creates a new read-only PageRefaultCounter from the provided VMO. The VMO must have the
    /// READ, MAP, and GET_PROPERTY rights.
    pub fn from_vmo_readonly(vmo: zx::Vmo) -> Result<Self, zx::Status> {
        if vmo.get_size()? < size_of::<AtomicU64>().try_into().unwrap() {
            return Err(zx::Status::INVALID_ARGS);
        }
        // SAFETY: all accesses to [storage] are synchronized (through an Atomic).
        let storage: MemoryMappedVmo = unsafe { MemoryMappedVmo::new_readonly(&vmo)? };
        let count_ptr: *const AtomicU64 =
            storage.get_object::<AtomicU64>(0).map_err(|_| zx::Status::INVALID_ARGS)?;
        Ok(PageRefaultCounter { vmo: vmo, _storage: storage, count_ptr })
    }

    pub fn increment(&self, count: u64, order: Ordering) {
        // SAFETY: `self.count_ptr` is non-null per construction, and valid as long as `_storage`
        // is valid.
        unsafe { &*self.count_ptr }.fetch_add(count, order);
    }

    pub fn read(&self, order: Ordering) -> u64 {
        // SAFETY: `self.count_ptr` is non-null per construction, and valid as long as `_storage`
        // is valid.
        unsafe { &*self.count_ptr }.load(order)
    }

    /// Returns a read-only handle for the backing VMO.
    pub fn readonly_vmo(&self) -> Result<zx::Vmo, zx::Status> {
        self.vmo.duplicate_handle(Rights::BASIC | Rights::READ | Rights::MAP | Rights::GET_PROPERTY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_refault_counter() {
        let counter = PageRefaultCounter::new().unwrap();

        let ro_vmo = counter.readonly_vmo().unwrap();
        let ro_counter = PageRefaultCounter::from_vmo_readonly(ro_vmo).unwrap();

        counter.increment(100, Ordering::SeqCst);
        assert_eq!(ro_counter.read(Ordering::SeqCst), 100);

        counter.increment(100, Ordering::SeqCst);
        assert_eq!(ro_counter.read(Ordering::SeqCst), 200);
    }
}
