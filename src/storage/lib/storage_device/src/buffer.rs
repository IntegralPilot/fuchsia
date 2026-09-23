// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffer_allocator::BufferAllocator as PoolBufferAllocator;
use std::borrow::Borrow;
use std::marker::PhantomData;
use std::ops::{Bound, Range, RangeBounds};
use std::slice::SliceIndex;
use storage_ptr_slice::{MutPtrByteSlice, PtrByteSlice};

pub use crate::buffer_allocator::{BufferFuture, TryAllocateBuffer};

#[cfg(target_os = "fuchsia")]
use zx::sys::zx_paddr_t;

/// An entity capable of reclaiming a memory buffer range when dropped.
pub trait BufferAllocator: Send + Sync + std::fmt::Debug + 'static {
    /// Frees or reclaims the specified memory range.
    fn free_buffer(&self, range: Range<usize>);

    /// Returns an identifier for this allocator based on its memory address.
    fn identifier(&self) -> usize {
        std::ptr::from_ref(self).addr()
    }

    /// Returns true if buffers produced by this allocator are trusted (unshared).
    fn is_trusted(&self) -> bool {
        false
    }

    /// Returns the underlying VMO if backed by a VMO and untrusted.
    ///
    /// If the allocator is trusted, this returns `None` to prevent external modification
    /// of memory that is assumed to be unshared.
    #[cfg(target_os = "fuchsia")]
    fn vmo(&self) -> Option<Arc<zx::Vmo>> {
        None
    }

    /// Returns the physical addresses for `range` if pinned, along with the contiguity.
    #[cfg(target_os = "fuchsia")]
    fn paddrs(&self, _range: &Range<usize>) -> Option<(&[zx_paddr_t], u64)> {
        None
    }
}

pub(super) fn round_down<T>(value: T, granularity: T) -> T
where
    T: num::Num + Copy,
{
    value - value % granularity
}

pub(super) fn round_up<T>(value: T, granularity: T) -> T
where
    T: num::Num + Copy,
{
    round_down(value + granularity - T::one(), granularity)
}

// Returns a range within a range.
// For example, subrange(100..200, 20..30) = 120..130.
fn subrange<R: RangeBounds<usize>>(source: &Range<usize>, bounds: &R) -> Range<usize> {
    let subrange = (match bounds.start_bound() {
        Bound::Included(&s) => source.start + s,
        Bound::Excluded(&s) => source.start + s + 1,
        Bound::Unbounded => source.start,
    })..(match bounds.end_bound() {
        Bound::Included(&e) => source.start + e + 1,
        Bound::Excluded(&e) => source.start + e,
        Bound::Unbounded => source.end,
    });
    assert!(subrange.end <= source.end);
    subrange
}

fn split_range(range: &Range<usize>, mid: usize) -> (Range<usize>, Range<usize>) {
    let l = range.end - range.start;
    let base = range.start;
    (base..base + mid, base + mid..base + l)
}

/// Buffer is a read-write buffer that can be used for I/O with the block device. They are created
/// by a BufferAllocator, and automatically deallocate themselves when they go out of scope.
///
/// Most usage will be on the unowned BufferRef and MutableBufferRef types, since these types are
/// used for Device::read and Device::write.
///
/// Buffers are always block-aligned (both in offset and length), but unaligned slices can be made
/// with the reference types. That said, the Device trait requires aligned BufferRef and
/// MutableBufferRef objects, so alignment must be restored by the time a device read/write is
/// requested.
///
/// For example, when writing an unaligned amount of data to the device, generally two Buffers
/// would need to be involved; the input Buffer could be used to write everything up to the last
/// block, and a second single-block alignment Buffer would be used to read-modify-update the last
/// block.
use std::sync::Arc;

#[derive(Debug)]
pub struct BufferImpl<'a, H: Borrow<A>, A: ?Sized + BufferAllocator> {
    slice: MutPtrByteSlice<'a>,
    range: Range<usize>,
    allocator: H,
    _phantom: PhantomData<fn() -> &'a A>,
}

pub type Buffer<'a> = BufferImpl<'a, &'a PoolBufferAllocator, PoolBufferAllocator>;
pub type OwnedBuffer = BufferImpl<'static, Arc<dyn BufferAllocator>, dyn BufferAllocator>;

// Alias for the traits which need to be satisfied for `subslice` and friends.
// This trait is automatically satisfied for most typical uses (a..b, a.., ..b, ..).
pub trait SliceRange: Clone + RangeBounds<usize> + SliceIndex<[u8], Output = [u8]> {}
impl<T> SliceRange for T where T: Clone + RangeBounds<usize> + SliceIndex<[u8], Output = [u8]> {}

impl<'a, H: Borrow<A>, A: ?Sized + BufferAllocator> BufferImpl<'a, H, A> {
    pub(super) fn new(slice: MutPtrByteSlice<'a>, range: Range<usize>, allocator: H) -> Self {
        assert_eq!(slice.len(), range.end - range.start);
        Self { slice, range, allocator, _phantom: PhantomData }
    }

    /// Takes a read-only reference to this buffer.
    pub fn as_ref(&self) -> BufferRef<'_> {
        self.subslice(..)
    }

    /// Takes a read-only reference to this buffer over `range` (which must be within the size of
    /// the buffer).
    pub fn subslice<R: SliceRange>(&self, range: R) -> BufferRef<'_> {
        let new_range = subrange(&self.range, &range);
        let relative_range =
            (new_range.start - self.range.start)..(new_range.end - self.range.start);
        let slice = self.slice.as_ptr_slice().subslice(relative_range);
        BufferRef {
            slice,
            start: new_range.start,
            end: new_range.end,
            allocator_id: self.allocator.borrow().identifier(),
            trusted: self.allocator.borrow().is_trusted(),
        }
    }

    /// Takes a read-write reference to this buffer.
    pub fn as_mut(&mut self) -> MutableBufferRef<'_> {
        self.subslice_mut(..)
    }

    /// Returns an `io::Write` adapter for this buffer.
    pub fn writer(&mut self) -> storage_ptr_slice::Writer<'_> {
        self.slice.reborrow().writer()
    }

    /// Takes a read-write reference to this buffer over `range` (which must be within the size of
    /// the buffer).
    pub fn subslice_mut<R: SliceRange>(&mut self, range: R) -> MutableBufferRef<'_> {
        let new_range = subrange(&self.range, &range);
        let relative_range =
            (new_range.start - self.range.start)..(new_range.end - self.range.start);
        let slice = self.slice.reborrow().subslice_mut(relative_range);
        MutableBufferRef {
            slice,
            range: new_range,
            allocator_id: self.allocator.borrow().identifier(),
            trusted: self.allocator.borrow().is_trusted(),
        }
    }

    /// Returns the buffer's capacity.
    pub fn len(&self) -> usize {
        self.range.end - self.range.start
    }

    /// Returns the physical addresses for DMA if this buffer is pinned by its allocator.
    #[cfg(target_os = "fuchsia")]
    pub fn paddrs(&self) -> Option<&[zx_paddr_t]> {
        self.allocator.borrow().paddrs(&self.range).map(|(paddrs, _)| paddrs)
    }

    /// Returns the contiguity used when pinning this buffer, if pinned by its allocator.
    #[cfg(target_os = "fuchsia")]
    pub fn contiguity(&self) -> Option<u64> {
        self.allocator.borrow().paddrs(&self.range).map(|(_, contig)| contig)
    }

    /// Returns the underlying VMO if the buffer is untrusted and backed by a VMO.
    ///
    /// Returns `None` if the buffer is trusted.
    #[cfg(target_os = "fuchsia")]
    pub fn vmo(&self) -> Option<Arc<zx::Vmo>> {
        self.allocator.borrow().vmo()
    }

    /// Returns a reference to the underlying data if the buffer is trusted.
    /// Returns None if the buffer is untrusted (shared with the driver).
    pub fn try_as_slice(&self) -> Option<&[u8]> {
        if self.allocator.borrow().is_trusted() {
            // SAFETY: The buffer is trusted (not shared), so no concurrent mutation can occur.
            Some(unsafe { std::slice::from_raw_parts(self.slice.as_ptr(), self.len()) })
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying data if the buffer is trusted.
    /// Returns None if the buffer is untrusted (shared with the driver).
    pub fn try_as_mut_slice(&mut self) -> Option<&mut [u8]> {
        if self.allocator.borrow().is_trusted() {
            // SAFETY: The buffer is trusted (not shared), so no concurrent mutation can occur.
            Some(unsafe { std::slice::from_raw_parts_mut(self.slice.as_mut_ptr(), self.len()) })
        } else {
            None
        }
    }

    /// Copies the contents of this buffer into `dest`.
    ///
    /// # Panics
    ///
    /// Panics if `dest.len() != self.len()`.
    pub fn copy_to_slice(&self, dest: &mut [u8]) {
        self.slice.as_ptr_slice().copy_to_slice(dest);
    }

    /// Copies the contents of `src` into this buffer.
    ///
    /// # Panics
    ///
    /// Panics if `src.len() != self.len()`.
    pub fn copy_from_slice(&mut self, src: &[u8]) {
        self.slice.copy_from_ptr_slice(src.into());
    }

    /// Copies the contents of `src` buffer into this buffer.
    ///
    /// # Panics
    ///
    /// Panics if `src.len() != self.len()`.
    pub fn copy_from_buffer(&mut self, src: BufferRef<'_>) {
        self.as_mut().copy_from_buffer(src);
    }

    /// Fills the buffer with `val`.
    pub fn fill(&mut self, val: u8) {
        self.slice.fill(val);
    }

    /// Returns the range in the underlying BufferSource that this buffer covers.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// Returns a reference to the allocator.
    pub fn allocator(&self) -> &A {
        self.allocator.borrow()
    }

    /// Returns the buffer's contents as a Vec.
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_ref().to_vec()
    }

    /// Appends the buffer's contents to `vec`.
    pub fn append_to(&self, vec: &mut Vec<u8>) {
        self.as_ref().append_to(vec)
    }

    /// Returns a raw pointer to the buffer's contents.
    pub fn as_ptr(&self) -> *const u8 {
        self.slice.as_ptr()
    }

    /// Returns a mutable raw pointer to the buffer's contents.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.slice.as_mut_ptr()
    }

    /// Returns a read-only pointer slice over the buffer.
    pub fn as_ptr_slice(&self) -> PtrByteSlice<'_> {
        self.slice.as_ptr_slice()
    }

    /// Returns a mutable pointer slice over the buffer.
    pub fn as_mut_ptr_slice(&mut self) -> MutPtrByteSlice<'_> {
        self.slice.reborrow()
    }
}

impl<'a, H: Borrow<A>, A: ?Sized + BufferAllocator> Drop for BufferImpl<'a, H, A> {
    fn drop(&mut self) {
        self.allocator.borrow().free_buffer(self.range.clone());
    }
}

/// BufferRef is an unowned, read-only view over a Buffer.
#[derive(Clone, Copy, Debug)]
pub struct BufferRef<'a> {
    slice: PtrByteSlice<'a>,
    start: usize, // Not range so that we get Copy.
    end: usize,
    /// Opaque identifier derived from the memory address of the `BufferAllocator`.
    /// Used internally to detect foreign buffers allocated from a different allocator.
    allocator_id: usize,
    trusted: bool,
}

impl<'a> BufferRef<'a> {
    /// Returns the buffer's capacity.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }

    #[cfg(target_os = "fuchsia")]
    pub(crate) fn allocator_id(&self) -> usize {
        self.allocator_id
    }

    /// Returns a reference to the underlying data if the buffer is trusted.
    /// Returns None if the buffer is untrusted (shared with the driver).
    pub fn try_as_slice(&self) -> Option<&[u8]> {
        if self.trusted {
            // SAFETY: The buffer is trusted (not shared), so no concurrent mutation can occur.
            Some(unsafe { std::slice::from_raw_parts(self.slice.as_ptr(), self.len()) })
        } else {
            None
        }
    }

    /// Slices and consumes this reference. See Buffer::subslice.
    pub fn subslice<R: SliceRange>(&self, range: R) -> BufferRef<'_> {
        let new_range = subrange(&self.range(), &range);
        let relative_range = (new_range.start - self.start)..(new_range.end - self.start);
        let slice = self.slice.subslice(relative_range);
        BufferRef {
            slice,
            start: new_range.start,
            end: new_range.end,
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        }
    }

    /// Splits at `mid` (included in the right child), yielding two BufferRefs.
    pub fn split_at(&self, mid: usize) -> (BufferRef<'a>, BufferRef<'a>) {
        let ranges = split_range(&self.range(), mid);
        let (left_slice, right_slice) = self.slice.split_at(mid);
        (
            BufferRef {
                slice: left_slice,
                start: ranges.0.start,
                end: ranges.0.end,
                allocator_id: self.allocator_id,
                trusted: self.trusted,
            },
            BufferRef {
                slice: right_slice,
                start: ranges.1.start,
                end: ranges.1.end,
                allocator_id: self.allocator_id,
                trusted: self.trusted,
            },
        )
    }

    /// Returns an iterator over byte chunks of up to `chunk_size` bytes.
    ///
    /// # Panics
    ///
    /// Panics if `chunk_size` is 0.
    pub fn chunks(self, chunk_size: usize) -> Chunks<'a> {
        Chunks {
            inner: self.slice.chunks(chunk_size),
            start: self.start,
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        }
    }

    /// Returns the range in the underlying BufferSource that this BufferRef covers.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// Copies the contents of this buffer into `dest`.
    ///
    /// # Panics
    ///
    /// Panics if `dest.len() != self.len()`.
    pub fn copy_to_slice(&self, dest: &mut [u8]) {
        self.slice.copy_to_slice(dest);
    }

    /// Returns the buffer's contents as a Vec.
    pub fn to_vec(&self) -> Vec<u8> {
        self.slice.to_vec()
    }

    /// Appends the buffer's contents to `vec`.
    pub fn append_to(&self, vec: &mut Vec<u8>) {
        self.slice.append_to(vec);
    }

    /// Returns a raw pointer to the buffer's contents.
    pub fn as_ptr(&self) -> *const u8 {
        self.slice.as_ptr()
    }

    /// Returns a read-only pointer slice over the buffer.
    pub fn as_ptr_slice(&self) -> PtrByteSlice<'a> {
        self.slice
    }
}

/// MutableBufferRef is an unowned, read-write view of a Buffer.
#[derive(Debug)]
pub struct MutableBufferRef<'a> {
    slice: MutPtrByteSlice<'a>,
    range: Range<usize>,
    /// Opaque identifier derived from the memory address of the `BufferAllocator`.
    /// Used internally to detect foreign buffers allocated from a different allocator.
    allocator_id: usize,
    trusted: bool,
}

impl<'a> MutableBufferRef<'a> {
    /// Returns the buffer's capacity.
    pub fn len(&self) -> usize {
        self.range.end - self.range.start
    }

    pub fn is_empty(&self) -> bool {
        self.range.end == self.range.start
    }

    #[cfg(target_os = "fuchsia")]
    pub(crate) fn allocator_id(&self) -> usize {
        self.allocator_id
    }

    /// Returns a read-only view of the buffer.
    pub fn as_ref(&self) -> BufferRef<'_> {
        BufferRef {
            slice: self.slice.as_ptr_slice(),
            start: self.range.start,
            end: self.range.end,
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        }
    }

    /// Consumes this reference and returns a read-only view.
    pub fn into_ref(self) -> BufferRef<'a> {
        BufferRef {
            slice: self.slice.into(),
            start: self.range.start,
            end: self.range.end,
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        }
    }

    /// Returns a reference to the underlying data if the buffer is trusted.
    /// Returns None if the buffer is untrusted (shared with the driver).
    pub fn try_as_slice(&self) -> Option<&[u8]> {
        if self.trusted {
            // SAFETY: The buffer is trusted (not shared), so no concurrent mutation can occur.
            Some(unsafe { std::slice::from_raw_parts(self.slice.as_ptr(), self.len()) })
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying data if the buffer is trusted.
    /// Returns None if the buffer is untrusted (shared with the driver).
    pub fn try_as_mut_slice(&mut self) -> Option<&mut [u8]> {
        if self.trusted {
            // SAFETY: The buffer is trusted (not shared), so no concurrent mutation can occur.
            Some(unsafe { std::slice::from_raw_parts_mut(self.slice.as_mut_ptr(), self.len()) })
        } else {
            None
        }
    }

    /// Reborrows this reference with a lesser lifetime. This mirrors the usual borrowing semantics
    /// (i.e. the borrow ends when the new reference goes out of scope), and exists so that a
    /// MutableBufferRef can be subsliced without consuming it.
    ///
    /// For example:
    ///    let mut buf: MutableBufferRef<'_> = ...;
    ///    {
    ///        let sub = buf.reborrow().subslice_mut(a..b);
    ///    }
    pub fn reborrow(&mut self) -> MutableBufferRef<'_> {
        MutableBufferRef {
            slice: self.slice.reborrow(),
            range: self.range.clone(),
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        }
    }

    /// Returns an `io::Write` adapter for this buffer.
    pub fn writer(&mut self) -> storage_ptr_slice::Writer<'_> {
        self.slice.reborrow().writer()
    }

    /// Slices this reference. See Buffer::subslice.
    pub fn subslice<R: SliceRange>(&self, range: R) -> BufferRef<'_> {
        let new_range = subrange(&self.range, &range);
        let relative_range =
            (new_range.start - self.range.start)..(new_range.end - self.range.start);
        let slice = self.slice.as_ptr_slice().subslice(relative_range);
        BufferRef {
            slice,
            start: new_range.start,
            end: new_range.end,
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        }
    }

    /// Slices and consumes this reference. See Buffer::subslice_mut.
    pub fn subslice_mut<R: SliceRange>(mut self, range: R) -> MutableBufferRef<'a> {
        let new_range = subrange(&self.range, &range);
        let relative_range =
            (new_range.start - self.range.start)..(new_range.end - self.range.start);
        self.slice = self.slice.subslice_mut(relative_range);
        self.range = new_range;
        self
    }

    /// Splits at `mid` (included in the right child), yielding two BufferRefs.
    pub fn split_at(&self, mid: usize) -> (BufferRef<'_>, BufferRef<'_>) {
        let ranges = split_range(&self.range, mid);
        let (left_slice, right_slice) = self.slice.as_ptr_slice().split_at(mid);
        (
            BufferRef {
                slice: left_slice,
                start: ranges.0.start,
                end: ranges.0.end,
                allocator_id: self.allocator_id,
                trusted: self.trusted,
            },
            BufferRef {
                slice: right_slice,
                start: ranges.1.start,
                end: ranges.1.end,
                allocator_id: self.allocator_id,
                trusted: self.trusted,
            },
        )
    }

    /// Consumes the reference and splits it at `mid` (included in the right child), yielding two
    /// MutableBufferRefs.
    pub fn split_at_mut(self, mid: usize) -> (MutableBufferRef<'a>, MutableBufferRef<'a>) {
        let ranges = split_range(&self.range, mid);
        let (left_slice, right_slice) = self.slice.split_at_mut(mid);
        (
            MutableBufferRef {
                slice: left_slice,
                range: ranges.0,
                allocator_id: self.allocator_id,
                trusted: self.trusted,
            },
            MutableBufferRef {
                slice: right_slice,
                range: ranges.1,
                allocator_id: self.allocator_id,
                trusted: self.trusted,
            },
        )
    }

    /// Returns the range in the underlying BufferSource that this MutableBufferRef covers.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// Copies the contents of this buffer into `dest`.
    ///
    /// # Panics
    ///
    /// Panics if `dest.len() != self.len()`.
    pub fn copy_to_slice(&self, dest: &mut [u8]) {
        self.slice.copy_to_slice(dest);
    }

    /// Copies the contents of `src` into this buffer.
    ///
    /// # Panics
    ///
    /// Panics if `src.len() != self.len()`.
    pub fn copy_from_slice(&mut self, src: &[u8]) {
        self.slice.copy_from_ptr_slice(src.into());
    }

    /// Copies the contents of `src` buffer into this buffer.
    ///
    /// # Panics
    ///
    /// Panics if `src.len() != self.len()`.
    pub fn copy_from_buffer(&mut self, src: BufferRef<'_>) {
        self.slice.copy_from_ptr_slice(src.as_ptr_slice());
    }

    /// Fills the buffer with `val`.
    pub fn fill(&mut self, val: u8) {
        self.slice.fill(val);
    }

    /// Returns the buffer's contents as a Vec.
    pub fn to_vec(&self) -> Vec<u8> {
        self.slice.to_vec()
    }

    /// Appends the buffer's contents to `vec`.
    pub fn append_to(&self, vec: &mut Vec<u8>) {
        self.slice.append_to(vec);
    }

    /// Returns a raw pointer to the buffer's contents.
    pub fn as_ptr(&self) -> *const u8 {
        self.slice.as_ptr()
    }

    /// Returns a mutable raw pointer to the buffer's contents.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.slice.as_mut_ptr()
    }

    /// Returns a read-only pointer slice over the buffer.
    pub fn as_ptr_slice(&self) -> PtrByteSlice<'_> {
        self.slice.as_ptr_slice()
    }

    /// Returns a mutable pointer slice over the buffer.
    pub fn as_mut_ptr_slice(&mut self) -> MutPtrByteSlice<'_> {
        self.slice.reborrow()
    }

    /// Returns an iterator over mutable byte chunks of up to `chunk_size` bytes.
    ///
    /// # Panics
    ///
    /// Panics if `chunk_size` is 0.
    pub fn chunks_mut(self, chunk_size: usize) -> ChunksMut<'a> {
        ChunksMut {
            inner: self.slice.into_chunks_mut(chunk_size),
            start: self.range.start,
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        }
    }

    /// Consumes this reference and returns a mutable pointer slice.
    pub fn into_mut_ptr_slice(self) -> MutPtrByteSlice<'a> {
        self.slice
    }
}

/// An iterator over slice chunks of a `BufferRef`.
#[derive(Debug)]
pub struct Chunks<'a> {
    inner: storage_ptr_slice::Chunks<'a>,
    start: usize,
    allocator_id: usize,
    trusted: bool,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = BufferRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let slice = self.inner.next()?;
        let len = slice.len();
        let start = self.start;
        self.start += len;
        Some(BufferRef {
            slice,
            start,
            end: start + len,
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for Chunks<'_> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl std::iter::FusedIterator for Chunks<'_> {}

/// An iterator over mutable slice chunks of a `MutableBufferRef`.
#[derive(Debug)]
pub struct ChunksMut<'a> {
    inner: storage_ptr_slice::ChunksMut<'a>,
    start: usize,
    allocator_id: usize,
    trusted: bool,
}

impl<'a> Iterator for ChunksMut<'a> {
    type Item = MutableBufferRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let slice = self.inner.next()?;
        let len = slice.len();
        let start = self.start;
        self.start += len;
        Some(MutableBufferRef {
            slice,
            range: start..start + len,
            allocator_id: self.allocator_id,
            trusted: self.trusted,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for ChunksMut<'_> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl std::iter::FusedIterator for ChunksMut<'_> {}

// SAFETY: BufferRef is a read-only view over allocator-managed memory. It does not allow
// mutation and behaves like `&[u8]`, which is Send and Sync.
unsafe impl Send for BufferRef<'_> {}
// SAFETY: See Send impl above.
unsafe impl Sync for BufferRef<'_> {}

// SAFETY: MutableBufferRef behaves like `&mut [u8]`. It enforces exclusivity (no overlapping
// views) and does not have interior mutability, making it safe to Send and Sync.
unsafe impl Send for MutableBufferRef<'_> {}
// SAFETY: See Send impl above.
unsafe impl Sync for MutableBufferRef<'_> {}

#[cfg(test)]
mod tests {
    use crate::buffer_allocator::{BufferAllocator, BufferSource};

    #[fuchsia::test]
    async fn test_chunks() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(512, source);
        let mut buf = allocator.allocate_buffer(1000).await;
        let init_data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        buf.as_mut().copy_from_slice(&init_data);

        let bref = buf.as_ref();
        let chunks: Vec<_> = bref.chunks(300).collect();
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].len(), 300);
        assert_eq!(chunks[1].len(), 300);
        assert_eq!(chunks[2].len(), 300);
        assert_eq!(chunks[3].len(), 100);

        let mut data = vec![0u8; 300];
        chunks[0].copy_to_slice(&mut data);
        assert_eq!(data, (0..300).map(|i| (i % 256) as u8).collect::<Vec<u8>>());

        let mut data_last = vec![0u8; 100];
        chunks[3].copy_to_slice(&mut data_last);
        assert_eq!(data_last, (900..1000).map(|i| (i % 256) as u8).collect::<Vec<u8>>());

        // Test exact multiple
        let bref_exact = buf.subslice(0..600);
        let chunks_exact: Vec<_> = bref_exact.chunks(200).collect();
        assert_eq!(chunks_exact.len(), 3);
        assert_eq!(chunks_exact.iter().map(|c| c.len()).collect::<Vec<_>>(), vec![200, 200, 200]);

        // Test empty buffer
        let bref_empty = buf.subslice(0..0);
        assert_eq!(bref_empty.chunks(100).count(), 0);
        assert_eq!(bref_empty.chunks(100).len(), 0);

        // Test chunk larger than buffer
        let chunks_large: Vec<_> = bref.chunks(2000).collect();
        assert_eq!(chunks_large.len(), 1);
        assert_eq!(chunks_large[0].len(), 1000);

        // Test ExactSizeIterator and size_hint
        let mut iter = bref.chunks(300);
        assert_eq!(iter.len(), 4);
        assert_eq!(iter.size_hint(), (4, Some(4)));
        assert_eq!(iter.next().unwrap().len(), 300);
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert_eq!(iter.next().unwrap().len(), 300);
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(iter.next().unwrap().len(), 300);
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));
        assert_eq!(iter.next().unwrap().len(), 100);
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert!(iter.next().is_none());
    }

    #[fuchsia::test]
    async fn test_chunks_mut() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(512, source);
        let mut buf = allocator.allocate_buffer(1000).await;

        for (i, mut chunk) in buf.as_mut().chunks_mut(300).enumerate() {
            chunk.fill(i as u8 + 1);
        }

        let mut data = vec![0u8; 1000];
        buf.copy_to_slice(&mut data);
        assert_eq!(&data[0..300], &[1u8; 300]);
        assert_eq!(&data[300..600], &[2u8; 300]);
        assert_eq!(&data[600..900], &[3u8; 300]);
        assert_eq!(&data[900..1000], &[4u8; 100]);

        // Test exact multiple
        let mut buf_exact = allocator.allocate_buffer(600).await;
        for (i, mut chunk) in buf_exact.as_mut().chunks_mut(200).enumerate() {
            chunk.fill((i + 10) as u8);
        }
        let mut data_exact = vec![0u8; 600];
        buf_exact.copy_to_slice(&mut data_exact);
        assert_eq!(&data_exact[0..200], &[10u8; 200]);
        assert_eq!(&data_exact[200..400], &[11u8; 200]);
        assert_eq!(&data_exact[400..600], &[12u8; 200]);

        // Test empty buffer
        let mut buf_empty = allocator.allocate_buffer(512).await;
        let empty_ref = buf_empty.subslice_mut(0..0);
        assert_eq!(empty_ref.chunks_mut(100).count(), 0);

        // Test ExactSizeIterator on chunks_mut
        let mut buf_exact_iter = allocator.allocate_buffer(1000).await;
        let mut iter = buf_exact_iter.as_mut().chunks_mut(300);
        assert_eq!(iter.len(), 4);
        assert_eq!(iter.size_hint(), (4, Some(4)));
        let mut c1 = iter.next().unwrap();
        c1.fill(0x11);
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        let mut c2 = iter.next().unwrap();
        c2.fill(0x22);
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));
        let mut c3 = iter.next().unwrap();
        c3.fill(0x33);
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));
        let mut c4 = iter.next().unwrap();
        c4.fill(0x44);
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert!(iter.next().is_none());

        let mut data_iter = vec![0u8; 1000];
        buf_exact_iter.copy_to_slice(&mut data_iter);
        assert_eq!(&data_iter[0..300], &[0x11; 300]);
        assert_eq!(&data_iter[300..600], &[0x22; 300]);
        assert_eq!(&data_iter[600..900], &[0x33; 300]);
        assert_eq!(&data_iter[900..1000], &[0x44; 100]);
    }
}
