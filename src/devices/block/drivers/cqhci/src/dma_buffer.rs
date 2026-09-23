// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx::sys::zx_paddr_t;

/// A mutable slice which can only be written to.
///
/// This is used as the argument to the callback in [`DmaBuffer::write`] to elide cache invalidation
/// before invoking the callback.
///
/// If a regular mutable slice were returned, the caller could read from that slice, and reading
/// from that slice may not be coherent without first invalidating the write cache.  Because the
/// caller can only write to the slice, the only requirement is that we flush the cached write after
/// invoking the callback.
pub struct WriteOnlySlice<'a, T>(&'a mut [T]);

impl<'a, T> WriteOnlySlice<'a, T> {
    #[inline(always)]
    pub fn set(&mut self, idx: usize, value: T) {
        self.0[idx] = value;
    }
}

/// A pinned region of memory suitable for DMA.
///
/// There are two variants of DmaBuffer:
///  - [`DiscontiguousDmaBuffer`], in which every page is pinned but not necessarily contiguous, and
///  - [`ContiguousDmaBuffer`], which is a single contiguous region.
pub trait DmaBuffer {
    /// Unpins the DmaBuffer, rendering it unusable for DMA.
    ///
    /// # Safety
    ///
    /// This MUST be called before dropping the DmaBuffer to avoid leaks, and MUST be called after
    /// the DmaBuffer will no longer be accessed by hardware to avoid stray DMA writes.
    unsafe fn unpin(&mut self) -> Result<(), zx::Status>;

    /// Returns the size in bytes of the DmaBuffer.
    fn size(&self) -> usize;

    /// Writes `count` elements of type `T` at `offset`.
    ///
    /// The memory region to be written is passed into `callback` as a [`WriteOnlySlice`] which the
    /// caller can directly modify.
    ///
    /// Once the callback returns, the writes will be committed by calling
    /// `zx_cache_flush(ZX_CACHE_FLUSH_DATA)`, which will ensure the writes are visible to external
    /// devices.
    ///
    /// # Safety
    ///
    /// This is unsafe because it takes a shared reference.  The caller is responsible for ensuring
    /// that the memory region being written to is uniquely accessed by the caller.
    unsafe fn write<T: zerocopy::IntoBytes + zerocopy::FromBytes + zerocopy::Immutable + Copy>(
        &self,
        offset: usize,
        count: usize,
        callback: impl FnOnce(WriteOnlySlice<'_, T>),
    ) -> Result<(), zx::Status>;
}

/// An implementation of [`DmaBuffer`] backed by a discontiguous memory region.
pub struct DiscontiguousDmaBuffer {
    vmar: zx::Vmar,
    _vmo: zx::Vmo,
    // The virtual address of the memory mapping.
    address: usize,
    pmt: Option<zx::Pmt>,
    // Each address points to a single page.
    phys_addresses: Vec<zx_paddr_t>,
    size: usize,
}

impl DiscontiguousDmaBuffer {
    /// Creates a new DicontiguousDmaBuffer for at least `size` bytes.
    pub fn new(vmar: zx::Vmar, bti: &zx::Bti, mut size: usize) -> Result<Self, zx::Status> {
        size = size.next_multiple_of(zx::system_get_page_size() as usize);
        let vmo = zx::Vmo::create(size as u64)?;
        let mut phys_addresses = vec![0; size / zx::system_get_page_size() as usize];
        let pmt = bti.pin(
            zx::BtiOptions::PERM_READ | zx::BtiOptions::PERM_WRITE,
            &vmo,
            0,
            size as u64,
            &mut phys_addresses[..],
        )?;
        let address =
            vmar.map(0, &vmo, 0, size, zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE)?;
        Ok(Self { vmar, _vmo: vmo, address, pmt: Some(pmt), phys_addresses, size })
    }

    /// Returns the physical address for the page which contains `vmo_offset`.
    pub fn phys_address_for(&self, vmo_offset: usize) -> zx_paddr_t {
        self.phys_addresses[vmo_offset / zx::system_get_page_size() as usize]
    }

    #[cfg(test)]
    pub fn vmo(&self) -> &zx::Vmo {
        &self._vmo
    }
}

impl DmaBuffer for DiscontiguousDmaBuffer {
    unsafe fn unpin(&mut self) -> Result<(), zx::Status> {
        if let Some(pmt) = self.pmt.take() {
            let res = unsafe { pmt.unpin() }.into();
            self.phys_addresses = vec![];
            res
        } else {
            Err(zx::Status::BAD_STATE)
        }
    }

    fn size(&self) -> usize {
        self.size
    }

    unsafe fn write<T: zerocopy::IntoBytes + zerocopy::FromBytes + zerocopy::Immutable + Copy>(
        &self,
        offset: usize,
        count: usize,
        callback: impl FnOnce(WriteOnlySlice<'_, T>),
    ) -> Result<(), zx::Status> {
        if self.pmt.is_none() {
            return Err(zx::Status::BAD_STATE);
        }
        let size = std::mem::size_of::<T>() * count;
        if offset.saturating_add(size) > self.size {
            return Err(zx::Status::INVALID_ARGS);
        }
        let ptr = (self.address + offset) as *mut u8;
        let buf = std::ptr::slice_from_raw_parts_mut(ptr, size);
        // SAFETY:  We checked that `buf` does not exceed the end of the memory region pointed to by
        // `self.address`.  We also know that we have a unique reference to the memory region
        // pointed to by `self.address`, since `self` is &mut.
        let bytes = unsafe { &mut *buf };
        let slice: &mut [T] = zerocopy::FromBytes::mut_from_bytes(bytes).unwrap();
        (callback)(WriteOnlySlice(slice));
        // SAFETY:  We checked that `ptr + size` does not exceed the end of the memory region
        // pointed to by `self.address`.
        unsafe { zx::ok(zx::sys::zx_cache_flush(ptr, size, zx::sys::ZX_CACHE_FLUSH_DATA)) }
    }
}

impl Drop for DiscontiguousDmaBuffer {
    fn drop(&mut self) {
        assert!(self.pmt.is_none(), "DmaBuffer was not unpinned");
        // SAFETY:  The memory pointed to by `self.address` is only accessible via methods on this
        // object, and no references to this object remain.  The memory is therefore safe to unmap.
        unsafe {
            let _ = self.vmar.unmap(self.address, self.size);
        }
    }
}

/// An implementation of [`DmaBuffer`] backed by a single contiguous memory region.
///
/// When possible, prefer to use [`DiscontiguousDmaBuffer`], to avoid consuming large contiguous
/// regions of physical memory.
pub struct ContiguousDmaBuffer(DiscontiguousDmaBuffer);

impl ContiguousDmaBuffer {
    /// Creates a new ContiguousDmaBuffer for at least `size` bytes.
    pub fn new(vmar: zx::Vmar, bti: &zx::Bti, mut size: usize) -> Result<Self, zx::Status> {
        size = size.next_multiple_of(zx::system_get_page_size() as usize);
        let vmo = zx::Vmo::create_contiguous(bti, size, 0)?;
        let mut phys_address = 0;
        let pmt = bti.pin(
            zx::BtiOptions::PERM_READ | zx::BtiOptions::PERM_WRITE | zx::BtiOptions::CONTIGUOUS,
            &vmo,
            0,
            size as u64,
            std::slice::from_mut(&mut phys_address),
        )?;
        let address =
            vmar.map(0, &vmo, 0, size, zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE)?;
        Ok(Self(DiscontiguousDmaBuffer {
            vmar,
            _vmo: vmo,
            address,
            pmt: Some(pmt),
            phys_addresses: vec![phys_address],
            size,
        }))
    }

    pub fn phys_address(&self) -> zx_paddr_t {
        self.0.phys_addresses[0]
    }

    #[cfg(test)]
    pub fn vmo(&self) -> &zx::Vmo {
        self.0.vmo()
    }
}

impl DmaBuffer for ContiguousDmaBuffer {
    unsafe fn unpin(&mut self) -> Result<(), zx::Status> {
        unsafe { self.0.unpin() }
    }

    fn size(&self) -> usize {
        self.0.size()
    }

    unsafe fn write<T: zerocopy::IntoBytes + zerocopy::FromBytes + zerocopy::Immutable + Copy>(
        &self,
        offset: usize,
        count: usize,
        callback: impl FnOnce(WriteOnlySlice<'_, T>),
    ) -> Result<(), zx::Status> {
        // SAFETY: Same rules apply.
        unsafe { self.0.write(offset, count, callback) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fake_bti::FakeBti;

    #[test]
    fn contiguous() {
        let bti = FakeBti::create().expect("Failed to create fake bti");
        bti.set_paddrs(&[0x9000]);

        let vmar = fuchsia_runtime::vmar_root_self()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("duplicate_handle failed");
        let mut dma =
            ContiguousDmaBuffer::new(vmar, &bti, 0x8000).expect("Failed to create DMA buffer");
        assert_eq!(dma.phys_address(), 0x9000);
        assert_eq!(dma.size(), 0x8000);

        type U32 = zerocopy::byteorder::little_endian::U32;
        unsafe {
            dma.write::<U32>(16, 2, |mut slice| {
                slice.set(0, U32::from(0x11223344));
                slice.set(1, U32::from(0xAABBCCDD));
            })
            .expect("write failed");
        }

        let data: Vec<U32> = dma.vmo().read_to_vec::<U32>(16, 2).expect("read failed");
        assert_eq!(data[0], U32::from(0x11223344));
        assert_eq!(data[1], U32::from(0xAABBCCDD));

        unsafe {
            dma.write::<U32>(18, 1, |mut slice| {
                slice.set(0, U32::from(0x55667788));
            })
            .expect("write failed");
        }

        let data: Vec<U32> = dma.vmo().read_to_vec::<U32>(16, 2).expect("read failed");
        assert_eq!(data[0], U32::from(0x77883344));
        assert_eq!(data[1], U32::from(0xAABB5566));

        unsafe {
            dma.unpin().expect("unpin failed");
        }
    }

    #[test]
    fn discontiguous() {
        let bti = FakeBti::create().expect("Failed to create fake bti");
        bti.set_paddrs(&[0x4000, 0x8000, 0xb000]);

        let vmar = fuchsia_runtime::vmar_root_self()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("duplicate_handle failed");
        let mut dma =
            DiscontiguousDmaBuffer::new(vmar, &bti, 0x3000).expect("Failed to create DMA buffer");
        assert_eq!(dma.phys_address_for(0), 0x4000);
        assert_eq!(dma.phys_address_for(0xfff), 0x4000);
        assert_eq!(dma.phys_address_for(0x1000), 0x8000);
        assert_eq!(dma.phys_address_for(0x1fff), 0x8000);
        assert_eq!(dma.phys_address_for(0x2000), 0xb000);
        assert_eq!(dma.phys_address_for(0x2fff), 0xb000);
        assert_eq!(dma.size(), 0x3000);

        unsafe {
            dma.unpin().expect("unpin failed");
        }
    }
}
