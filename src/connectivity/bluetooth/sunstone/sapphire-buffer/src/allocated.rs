// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(feature = "alloc")]

extern crate alloc;

use alloc::boxed::Box;
use core::cell::{Cell, UnsafeCell};

use crate::{Buffer, BufferAccessor, Handle, OutOfBounds};

/// A heap-allocated byte buffer managed by a [`StdVecBufferProvider`].
struct AllocatedBuffer {
    offset: Cell<usize>,
    len: Cell<usize>,
    data: UnsafeCell<alloc::vec::Vec<u8>>,
}

/// A buffer provider that allocates new buffers dynamically on the heap using standard vectors.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdVecBufferProvider;

impl StdVecBufferProvider {
    /// Constructs a new [`StdVecBufferProvider`].
    pub const fn new() -> Self {
        Self
    }

    /// Helper to safely inspect the allocated buffer for a handle.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `handle` was minted by `acquire_buffer` / `adopt_as_buffer`.
    #[inline(always)]
    unsafe fn get_buffer<'h>(&self, handle: &'h Handle<'_>) -> &'h AllocatedBuffer {
        let raw = handle.get();
        // SAFETY: raw is a valid pointer to AllocatedBuffer acquired by acquire_buffer / adopt_as_buffer.
        unsafe { &*(raw as *const AllocatedBuffer) }
    }
}

impl BufferAccessor for StdVecBufferProvider {
    unsafe fn len(&self, handle: &Handle<'_>) -> usize {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        buffer.len.get()
    }

    unsafe fn capacity(&self, handle: &Handle<'_>) -> usize {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        // SAFETY: The allocated heap vector length is constant for the lifetime of AllocatedBuffer.
        unsafe { (&*buffer.data.get()).len() }
    }

    unsafe fn truncate_front(
        &self,
        handle: &mut Handle<'_>,
        offset: usize,
    ) -> Result<(), OutOfBounds> {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        let current_len = buffer.len.get();
        if offset > current_len {
            return Err(OutOfBounds);
        }
        let current_offset = buffer.offset.get();
        buffer.offset.set(current_offset + offset);
        buffer.len.set(current_len - offset);
        Ok(())
    }

    unsafe fn truncate_back(
        &self,
        handle: &mut Handle<'_>,
        size: usize,
    ) -> Result<(), OutOfBounds> {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        let current_len = buffer.len.get();
        if size > current_len {
            return Err(OutOfBounds);
        }
        buffer.len.set(current_len - size);
        Ok(())
    }

    unsafe fn reclaim_front(
        &self,
        handle: &mut Handle<'_>,
        size: usize,
    ) -> Result<(), OutOfBounds> {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        let current_offset = buffer.offset.get();
        if size > current_offset {
            return Err(OutOfBounds);
        }
        let current_len = buffer.len.get();
        buffer.offset.set(current_offset - size);
        buffer.len.set(current_len + size);
        Ok(())
    }

    unsafe fn reclaim_back(&self, handle: &mut Handle<'_>, size: usize) -> Result<(), OutOfBounds> {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        let current_offset = buffer.offset.get();
        let current_len = buffer.len.get();
        // SAFETY: We hold exclusive mutable access to the active buffer
        let max_limit = unsafe { (&*buffer.data.get()).len() };
        if size > max_limit - (current_offset + current_len) {
            return Err(OutOfBounds);
        }
        buffer.len.set(current_len + size);
        Ok(())
    }

    unsafe fn reset_view(&self, handle: &mut Handle<'_>) {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        buffer.offset.set(0);
        // SAFETY: We hold exclusive mutable access to the active buffer
        let capacity = unsafe { (&*buffer.data.get()).len() };
        buffer.len.set(capacity);
    }

    unsafe fn as_slice<'b>(&self, handle: &'b Handle<'_>) -> &'b [u8] {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        let offset = buffer.offset.get();
        let len = buffer.len.get();
        // SAFETY: We hold exclusive access to the active buffer
        let slice = unsafe { &*buffer.data.get() };
        &slice[offset..offset + len]
    }

    unsafe fn as_mut_slice<'b>(&self, handle: &'b mut Handle<'_>) -> &'b mut [u8] {
        // SAFETY: The handle contract guarantees handle points to a valid AllocatedBuffer.
        let buffer = unsafe { self.get_buffer(handle) };
        let offset = buffer.offset.get();
        let len = buffer.len.get();
        // SAFETY: We hold exclusive mutable access to the active buffer
        let slice = unsafe { &mut *buffer.data.get() };
        &mut slice[offset..offset + len]
    }

    unsafe fn release(&self, handle: &mut Handle<'_>) {
        let raw = handle.get();
        // SAFETY: raw is the raw pointer to Box<AllocatedBuffer> acquired by acquire_buffer / adopt_as_buffer
        unsafe {
            let _ = Box::from_raw(raw as *mut AllocatedBuffer);
        }
    }
}

impl StdVecBufferProvider {
    /// Allocates a new heap-backed buffer of the requested `size` in bytes.
    pub fn acquire_buffer<'a>(&'a self, size: usize) -> Buffer<'a> {
        let data = alloc::vec![0u8; size];

        let buffer = Box::new(AllocatedBuffer {
            offset: Cell::new(0),
            len: Cell::new(size),
            data: UnsafeCell::new(data),
        });

        let raw = Box::into_raw(buffer) as usize;
        // SAFETY: raw is a valid, unique handle for this accessor per UnboundHandle safety contract.
        let unbound = unsafe { crate::UnboundHandle::new(raw) };
        // SAFETY: unbound handle was minted by self as BufferAccessor for lease `'a`.
        unsafe { Buffer::new(self as &'a dyn BufferAccessor, unbound) }
    }

    /// Adopts an existing heap vector into a new buffer view managed by this provider.
    pub fn adopt_as_buffer<'a>(&'a self, value: alloc::vec::Vec<u8>) -> Buffer<'a> {
        let size = value.len();
        let buffer = Box::new(AllocatedBuffer {
            offset: Cell::new(0),
            len: Cell::new(size),
            data: UnsafeCell::new(value),
        });

        let raw = Box::into_raw(buffer) as usize;
        // SAFETY: raw is a valid, unique handle for this accessor per UnboundHandle safety contract.
        let unbound = unsafe { crate::UnboundHandle::new(raw) };
        // SAFETY: unbound handle was minted by self as BufferAccessor for lease `'a`.
        unsafe { Buffer::new(self as &'a dyn BufferAccessor, unbound) }
    }

    /// Attempts to extract the underlying heap vector and active view range from a [`Buffer`].
    ///
    /// Returns `Ok((vec, range))` where `range` indicates the active slice view bounds within `vec`.
    /// If `buffer` was not allocated by a `StdVecBufferProvider`, returns `Err(buffer)`.
    pub fn extract_as_vec<'a>(
        &self,
        buffer: Buffer<'a>,
    ) -> Result<(alloc::vec::Vec<u8>, core::ops::Range<usize>), Buffer<'a>> {
        let (accessor, unbound) = buffer.into_parts();
        if (&*accessor as &dyn core::any::Any).is::<Self>() {
            // SAFETY: `accessor` is a `StdVecBufferProvider`, so `unbound` was allocated by `StdVecBufferProvider`.
            // consuming `unbound` transfers ownership of the handle without double-drop or leak.
            let raw = unsafe { unbound.into_raw() };
            let dyn_buf = unsafe { Box::from_raw(raw as *mut AllocatedBuffer) };
            let offset = dyn_buf.offset.get();
            let len = dyn_buf.len.get();
            Ok((dyn_buf.data.into_inner(), offset..offset + len))
        } else {
            // SAFETY: unbound handle was originally minted by accessor for lease `'a`.
            Err(unsafe { Buffer::new(accessor, unbound) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "UnboundHandle dropped without binding to a Buffer")]
    fn test_unbound_handle_drop_panic() {
        let _unbound = unsafe { crate::UnboundHandle::new(42) };
    }

    #[test]
    fn test_allocated_basic_acquisition_and_release() {
        let allocator = StdVecBufferProvider::default();
        let mut buf1 = allocator.acquire_buffer(64);
        let mut buf2 = allocator.acquire_buffer(128);

        assert_eq!(buf1.capacity(), 64);
        assert_eq!(buf2.capacity(), 128);

        // Write and check data
        buf1.as_mut().fill(42);
        buf2.as_mut().fill(24);

        assert_eq!(buf1.as_mut()[0], 42);
        assert_eq!(buf1.as_mut()[63], 42);
        assert_eq!(buf2.as_mut()[0], 24);
        assert_eq!(buf2.as_mut()[127], 24);
    }

    #[test]
    fn test_allocated_view_manipulations() {
        let allocator = StdVecBufferProvider::default();
        let mut buf = allocator.acquire_buffer(100);
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
    fn test_allocated_out_of_bounds() {
        let allocator = StdVecBufferProvider::default();
        let mut buf = allocator.acquire_buffer(50);

        assert_eq!(buf.truncate_front(60), Err(OutOfBounds));
        assert_eq!(buf.truncate_back(60), Err(OutOfBounds));

        buf.truncate_front(20).unwrap();
        assert_eq!(buf.reclaim_front(30), Err(OutOfBounds));

        buf.truncate_back(20).unwrap();
        assert_eq!(buf.reclaim_back(40), Err(OutOfBounds));
    }

    #[test]
    fn test_allocated_zero_size_acquisition() {
        let allocator = StdVecBufferProvider::default();
        let mut buf = allocator.acquire_buffer(0);
        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.as_mut().len(), 0);
    }

    #[test]
    fn test_buffer_extractor() {
        use crate::storage::StorageBufferProvider;
        use sapphire_collections::storage::storages::InlineStorage;

        let provider = StdVecBufferProvider::default();
        let mut storage_provider = StorageBufferProvider::new(InlineStorage::<[usize; 128]>::new());

        let mut buf = provider.acquire_buffer(10);
        buf.as_mut_slice().copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let ptr_before = buf.as_slice().as_ptr();

        // Successful extraction from allocated provider: address and contents are stable
        let (vec, range) = provider.extract_as_vec(buf).unwrap();
        assert_eq!(vec.as_ptr(), ptr_before);
        assert_eq!(&vec[range], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

        // Acquiring from non-StdVecBufferProvider and attempting extract_as_vec must return Err(buf)
        let mut storage_buf = storage_provider.acquire_only_buffer(20).unwrap();
        storage_buf.as_mut_slice().fill(0xAA);
        let storage_ptr_before = storage_buf.as_slice().as_ptr();

        let returned_buf = provider.extract_as_vec(storage_buf).err().unwrap();
        assert_eq!(returned_buf.as_slice().as_ptr(), storage_ptr_before);
        assert_eq!(returned_buf.capacity(), 20);
        assert_eq!(returned_buf.as_slice(), &[0xAA; 20]);
    }

    #[test]
    fn test_buffer_adopter() {
        let provider = StdVecBufferProvider::default();
        let src = alloc::vec![10, 20, 30, 40, 50];
        let ptr_before = src.as_ptr();

        let mut buf = provider.adopt_as_buffer(src);
        assert_eq!(buf.capacity(), 5);
        assert_eq!(buf.as_slice(), &[10, 20, 30, 40, 50]);
        assert_eq!(buf.as_slice().as_ptr(), ptr_before);

        buf.as_mut_slice()[2] = 99;
        buf.truncate_front(1).unwrap();
        buf.truncate_back(1).unwrap();

        let (extracted, range) = provider.extract_as_vec(buf).unwrap();
        assert_eq!(extracted.as_ptr(), ptr_before);
        assert_eq!(range, 1..4);
        assert_eq!(&extracted[range], &[20, 99, 40]);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Op {
        TruncateFront(usize),
        TruncateBack(usize),
        ReclaimFront(usize),
        ReclaimBack(usize),
        Reset,
    }

    fn op_strategy(max_val: usize) -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..max_val).prop_map(Op::TruncateFront),
            (0..max_val).prop_map(Op::TruncateBack),
            (0..max_val).prop_map(Op::ReclaimFront),
            (0..max_val).prop_map(Op::ReclaimBack),
            Just(Op::Reset),
        ]
    }

    proptest! {
        #[test]
        fn test_allocated_view_differential(
            alloc_size in 1usize..256,
            ops in prop::collection::vec(op_strategy(300), 1..50),
        ) {
            let allocator = StdVecBufferProvider::default();

            let mut buf = allocator.acquire_buffer(alloc_size);

            let mut ref_offset = 0;
            let mut ref_len = alloc_size;
            let backing_size = alloc_size;

            // Fill with tracking sequence
            for (i, byte) in buf.as_mut().iter_mut().enumerate() {
                *byte = (i % 256) as u8;
            }

            for op in ops {
                match op {
                    Op::TruncateFront(amt) => {
                        let res = buf.truncate_front(amt);
                        if amt <= ref_len {
                            res.unwrap();
                            ref_offset += amt;
                            ref_len -= amt;
                        } else {
                            assert_eq!(res, Err(OutOfBounds));
                        }
                    }
                    Op::TruncateBack(amt) => {
                        let res = buf.truncate_back(amt);
                        if amt <= ref_len {
                            res.unwrap();
                            ref_len -= amt;
                        } else {
                            assert_eq!(res, Err(OutOfBounds));
                        }
                    }
                    Op::ReclaimFront(amt) => {
                        let res = buf.reclaim_front(amt);
                        if amt <= ref_offset {
                            res.unwrap();
                            ref_offset -= amt;
                            ref_len += amt;
                        } else {
                            assert_eq!(res, Err(OutOfBounds));
                        }
                    }
                    Op::ReclaimBack(amt) => {
                        let res = buf.reclaim_back(amt);
                        if amt <= backing_size - (ref_offset + ref_len) {
                            res.unwrap();
                            ref_len += amt;
                        } else {
                            assert_eq!(res, Err(OutOfBounds));
                        }
                    }
                    Op::Reset => {
                        buf.reset_view();
                        ref_offset = 0;
                        ref_len = backing_size;
                    }
                }

                let slice = buf.as_mut();
                assert_eq!(slice.len(), ref_len);
                for (i, &byte) in slice.iter().enumerate() {
                    assert_eq!(byte, ((ref_offset + i) % 256) as u8);
                }
            }
        }
    }
}
