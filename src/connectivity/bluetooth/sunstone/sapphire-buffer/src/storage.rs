// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::alloc::Layout;
use core::cell::UnsafeCell;
use sapphire_collections::storage::Storage;

use crate::{Buffer, BufferAccessor, Handle, OutOfBounds};

/// Header stored alongside buffer memory in a [`Storage`] backend.
#[repr(C)]
struct StorageHeader<H> {
    offset: usize,
    len: usize,
    capacity: usize,
    handle: H,
}

#[inline]
fn compute_header_and_buffer_layout<H>(buffer_size: usize) -> Result<(Layout, usize), ()> {
    let header_layout = Layout::new::<StorageHeader<H>>();
    let buffer_layout = Layout::array::<u8>(buffer_size).map_err(|_| ())?;
    let (full_layout, buffer_offset) = header_layout.extend(buffer_layout).map_err(|_| ())?;
    Ok((full_layout.pad_to_align(), buffer_offset))
}

/// Adapts any [`Storage`] backend into a [`BufferAccessor`] provider.
pub struct StorageBufferProvider<S> {
    storage: UnsafeCell<S>,
}

impl<S> StorageBufferProvider<S> {
    /// Constructs a new [`StorageBufferProvider`] wrapping `storage`.
    pub const fn new(storage: S) -> Self {
        Self { storage: UnsafeCell::new(storage) }
    }
}

impl<S: Storage + 'static> StorageBufferProvider<S> {
    /// Helper to safely inspect the header for a handle.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `handle` was minted by `acquire_only_buffer` for this provider.
    #[inline(always)]
    unsafe fn header<'h>(handle: &'h Handle<'_>) -> &'h StorageHeader<S::Handle> {
        let raw = handle.get();
        // SAFETY: `raw` points to a valid `StorageHeader` allocated by `acquire_only_buffer`.
        unsafe { &*(raw as *const StorageHeader<S::Handle>) }
    }

    /// Helper to safely mutably inspect the header for an exclusive handle.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `handle` was minted by `acquire_only_buffer` for this provider
    /// and that the caller holds an exclusive borrow (`&mut Handle`).
    #[inline(always)]
    unsafe fn header_mut<'h>(handle: &'h mut Handle<'_>) -> &'h mut StorageHeader<S::Handle> {
        let raw = handle.get();
        // SAFETY: `raw` points to a valid `StorageHeader` allocated by `acquire_only_buffer`.
        unsafe { &mut *(raw as *mut StorageHeader<S::Handle>) }
    }

    /// Acquires a single exclusive buffer of the requested `size` from the underlying storage.
    ///
    /// Returns `None` if the requested size cannot be satisfied by the storage backend.
    pub fn acquire_only_buffer<'a>(&'a mut self, size: usize) -> Option<Buffer<'a>> {
        // SAFETY: By `&mut self`, self is mutably borrowed exclusively.
        let storage = unsafe { &mut *self.storage.get() };
        let (full_layout, buffer_offset) =
            compute_header_and_buffer_layout::<S::Handle>(size).ok()?;
        let (handle, _) = storage.allocate(full_layout).ok()?;
        // SAFETY: resolve returns a valid pointer to the allocation for `handle`.
        let ptr = unsafe { storage.resolve(handle).as_ptr() as *mut StorageHeader<S::Handle> };

        // SAFETY: ptr points to newly allocated, aligned memory large enough for StorageHeader and size bytes of payload.
        unsafe {
            core::ptr::write(ptr, StorageHeader { offset: 0, len: size, capacity: size, handle });
            core::ptr::write_bytes((ptr as *mut u8).add(buffer_offset), 0u8, size);
        }

        let raw = ptr as usize;
        // SAFETY: raw is a unique pointer to the allocated header and buffer bytes.
        let unbound = unsafe { crate::UnboundHandle::new(raw) };
        // SAFETY: unbound handle was minted by self as BufferAccessor for lease `'a`.
        Some(unsafe { Buffer::new(self as &'a dyn BufferAccessor, unbound) })
    }
}

impl<S: Storage + 'static> BufferAccessor for StorageBufferProvider<S> {
    unsafe fn len(&self, handle: &Handle<'_>) -> usize {
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header(handle) };
        header.len
    }

    unsafe fn capacity(&self, handle: &Handle<'_>) -> usize {
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header(handle) };
        header.capacity
    }

    unsafe fn truncate_front(
        &self,
        handle: &mut Handle<'_>,
        offset: usize,
    ) -> Result<(), OutOfBounds> {
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header_mut(handle) };
        let current_len = header.len;
        if offset > current_len {
            return Err(OutOfBounds);
        }
        header.offset += offset;
        header.len -= offset;
        Ok(())
    }

    unsafe fn truncate_back(
        &self,
        handle: &mut Handle<'_>,
        size: usize,
    ) -> Result<(), OutOfBounds> {
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header_mut(handle) };
        let current_len = header.len;
        if size > current_len {
            return Err(OutOfBounds);
        }
        header.len -= size;
        Ok(())
    }

    unsafe fn reclaim_front(
        &self,
        handle: &mut Handle<'_>,
        size: usize,
    ) -> Result<(), OutOfBounds> {
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header_mut(handle) };
        let current_offset = header.offset;
        if size > current_offset {
            return Err(OutOfBounds);
        }
        header.offset -= size;
        header.len += size;
        Ok(())
    }

    unsafe fn reclaim_back(&self, handle: &mut Handle<'_>, size: usize) -> Result<(), OutOfBounds> {
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header_mut(handle) };
        let available = header.capacity - (header.offset + header.len);
        if size > available {
            return Err(OutOfBounds);
        }
        header.len += size;
        Ok(())
    }

    unsafe fn reset_view(&self, handle: &mut Handle<'_>) {
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header_mut(handle) };
        header.offset = 0;
        header.len = header.capacity;
    }

    unsafe fn as_slice<'b>(&self, handle: &'b Handle<'_>) -> &'b [u8] {
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header(handle) };
        let raw = handle.get();
        let (_, buffer_offset) =
            compute_header_and_buffer_layout::<S::Handle>(header.capacity).unwrap();
        // SAFETY: raw + buffer_offset + header.offset points to valid memory of header.len bytes.
        unsafe {
            let start = (raw as *const u8).add(buffer_offset).add(header.offset);
            core::slice::from_raw_parts(start, header.len)
        }
    }

    unsafe fn as_mut_slice<'b>(&self, handle: &'b mut Handle<'_>) -> &'b mut [u8] {
        let raw = handle.get();
        // SAFETY: The handle lifetime contract guarantees that handle points to a valid StorageHeader.
        let header = unsafe { Self::header_mut(handle) };
        let (_, buffer_offset) =
            compute_header_and_buffer_layout::<S::Handle>(header.capacity).unwrap();
        // SAFETY: raw + buffer_offset + header.offset points to exclusively leased memory of header.len bytes.
        unsafe {
            let start = (raw as *mut u8).add(buffer_offset).add(header.offset);
            core::slice::from_raw_parts_mut(start, header.len)
        }
    }

    unsafe fn release(&self, handle: &mut Handle<'_>) {
        let raw = handle.get();
        // SAFETY: Reading the header out of raw is safe because this is the final release call.
        let header = unsafe { core::ptr::read(raw as *const StorageHeader<S::Handle>) };
        let (full_layout, _) =
            compute_header_and_buffer_layout::<S::Handle>(header.capacity).unwrap();

        // SAFETY: `acquire_only_buffer` exclusively borrowed self, ensuring no aliasing references can access storage concurrently.
        let storage = unsafe { &mut *self.storage.get() };
        // SAFETY: header.handle was allocated with full_layout by this storage.
        unsafe {
            storage.deallocate(full_layout, header.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sapphire_collections::storage::storages::InlineStorage;

    #[test]
    fn test_storage_single_buffer_provider() {
        let mut provider = StorageBufferProvider::new(InlineStorage::<[usize; 128]>::new());
        let mut buf = provider.acquire_only_buffer(100).unwrap();
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice().len(), 100);

        // Write and slice
        buf.as_mut().fill(0x55);
        assert_eq!(buf.as_slice()[0], 0x55);
        assert_eq!(buf.as_slice()[99], 0x55);

        // Truncate front
        buf.truncate_front(10).unwrap();
        assert_eq!(buf.len(), 90);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice().len(), 90);

        // Truncate back by 40 -> leaves 50
        buf.truncate_back(40).unwrap();
        assert_eq!(buf.len(), 50);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice().len(), 50);

        // Reset view
        buf.reset_view();
        assert_eq!(buf.len(), 100);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice().len(), 100);

        // Drop buffer (triggering release and deallocation)
        drop(buf);

        // Acquire again to verify storage is reusable
        let mut buf2 = provider.acquire_only_buffer(200).unwrap();
        assert_eq!(buf2.capacity(), 200);
        buf2.as_mut().fill(0xAA);
        assert_eq!(buf2.as_slice()[0], 0xAA);
    }

    #[test]
    fn test_storage_view_manipulations() {
        let mut provider = StorageBufferProvider::new(InlineStorage::<[usize; 128]>::new());
        let mut buf = provider.acquire_only_buffer(100).unwrap();
        for i in 0..100 {
            buf.as_mut()[i] = i as u8;
        }

        buf.truncate_front(10).unwrap();
        assert_eq!(buf.len(), 90);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice()[0], 10);

        buf.truncate_back(20).unwrap();
        assert_eq!(buf.len(), 70);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice()[69], 79);

        buf.reclaim_front(5).unwrap();
        assert_eq!(buf.len(), 75);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice()[0], 5);

        buf.reclaim_back(10).unwrap();
        assert_eq!(buf.len(), 85);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice()[84], 89);

        buf.reset_view();
        assert_eq!(buf.len(), 100);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.as_slice()[0], 0);
        assert_eq!(buf.as_slice()[99], 99);
    }

    #[test]
    fn test_storage_out_of_bounds() {
        let mut provider = StorageBufferProvider::new(InlineStorage::<[usize; 128]>::new());
        let mut buf = provider.acquire_only_buffer(50).unwrap();

        assert_eq!(buf.truncate_front(60), Err(OutOfBounds));
        assert_eq!(buf.truncate_back(60), Err(OutOfBounds));

        buf.truncate_front(20).unwrap();
        assert_eq!(buf.reclaim_front(30), Err(OutOfBounds));

        buf.truncate_back(20).unwrap();
        assert_eq!(buf.reclaim_back(40), Err(OutOfBounds));
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn test_global_storage_single_buffer_provider() {
        use sapphire_collections::storage::storages::Global;
        let mut provider = StorageBufferProvider::new(Global);
        let mut buf = provider.acquire_only_buffer(64).unwrap();
        assert_eq!(buf.capacity(), 64);
        buf.as_mut().fill(0x42);
        assert_eq!(buf.as_slice()[0], 0x42);
    }
}
