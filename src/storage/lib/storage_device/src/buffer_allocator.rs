// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffer::{
    Buffer, BufferAllocator as BufferAllocatorTrait, OwnedBuffer, round_down, round_up,
};
use event_listener::{Event, EventListener, Listener as _};
use fuchsia_sync::Mutex;
use futures::{Future, FutureExt as _};
use std::collections::BTreeMap;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

#[cfg(target_os = "fuchsia")]
mod buffer_source {
    use fuchsia_runtime::vmar_root_self;
    use std::ops::Range;
    use std::sync::Arc;
    use storage_ptr_slice::MutPtrByteSlice;

    /// A buffer source backed by a VMO.
    #[derive(Debug)]
    pub struct BufferSource {
        base: *mut u8,
        size: usize,
        vmo: Arc<zx::Vmo>,
        trusted: bool,
    }

    // SAFETY: This is required for the *mut u8 which is just the base address of the VMO mapping
    // and doesn't stop us making BufferSource Send and Sync.
    unsafe impl Send for BufferSource {}
    unsafe impl Sync for BufferSource {}

    impl BufferSource {
        pub fn new(size: usize) -> Self {
            Self::new_internal(size, false)
        }

        pub fn new_trusted(size: usize) -> Self {
            Self::new_internal(size, true)
        }

        fn new_internal(size: usize, trusted: bool) -> Self {
            let mut vmo = zx::Vmo::create(size as u64).unwrap();
            if trusted {
                // We strip the TRANSFER right to prevent the VMO handle from being transferred
                // or duplicated to another process (such as a block device driver). Because the
                // VMO cannot be shared remotely, we guarantee that this memory is strictly
                // private and cannot be accessed or modified concurrently by hardware or
                // drivers. This allows us to safely hand out standard Rust references (&[u8]
                // and &mut [u8]) without risking undefined behavior from aliasing or concurrent
                // mutation.
                let rights = zx::Rights::VMO_DEFAULT & !zx::Rights::TRANSFER;
                vmo = vmo.replace_handle(rights).unwrap();
            }
            let vmo = Arc::new(vmo);
            let name = zx::Name::new("transfer-buf").unwrap();
            vmo.set_name(&name).unwrap();
            let flags = zx::VmarFlags::PERM_READ
                | zx::VmarFlags::PERM_WRITE
                | zx::VmarFlags::MAP_RANGE
                | zx::VmarFlags::REQUIRE_NON_RESIZABLE;
            let base = vmar_root_self().map(0, &vmo, 0, size, flags).unwrap() as *mut u8;
            Self { base, size, vmo, trusted }
        }

        pub fn is_trusted(&self) -> bool {
            self.trusted
        }

        pub fn slice(&self) -> *mut [u8] {
            std::ptr::slice_from_raw_parts_mut(self.base, self.size)
        }

        pub fn size(&self) -> usize {
            self.size
        }

        pub fn vmo(&self) -> &Arc<zx::Vmo> {
            &self.vmo
        }

        /// Returns a mutable pointer slice for the given range.
        ///
        /// # Safety
        ///
        /// The caller must ensure that no other active references or pointer slices overlap with
        /// this range.
        pub(crate) unsafe fn subslice_ptr(&self, range: &Range<usize>) -> MutPtrByteSlice<'_> {
            assert!(range.start < self.size && range.end <= self.size);
            // SAFETY: The base pointer is valid for `size` bytes, and `range` is within bounds.
            // The caller guarantees exclusivity.
            unsafe {
                MutPtrByteSlice::new(std::ptr::slice_from_raw_parts_mut(
                    self.base.add(range.start),
                    range.len(),
                ))
            }
        }

        /// Returns a mutable pointer slice with an arbitrary lifetime `'a` for the given range.
        ///
        /// # Safety
        ///
        /// The caller must ensure that no other active references or pointer slices overlap with
        /// this range, and that `self` remains valid and mapped in memory for the entire lifetime
        /// `'a`.
        pub(crate) unsafe fn subslice_ptr_unbounded<'a>(
            &self,
            range: &Range<usize>,
        ) -> MutPtrByteSlice<'a> {
            assert!(range.start < self.size && range.end <= self.size);
            // SAFETY: The base pointer is valid for `size` bytes, and `range` is within bounds.
            // The caller guarantees exclusivity and memory liveness for `'a`.
            unsafe {
                MutPtrByteSlice::new(std::ptr::slice_from_raw_parts_mut(
                    self.base.add(range.start),
                    range.len(),
                ))
            }
        }

        /// Commits the range in memory to avoid future page faults.
        pub fn commit_range(&self, range: Range<usize>) -> Result<(), zx::Status> {
            self.vmo.op_range(zx::VmoOp::COMMIT, range.start as u64, range.len() as u64)
        }

        /// Zeroes out the range so the kerne can reclaim the pages.
        ///
        /// # Safety
        ///
        /// The range must not be allocated.
        pub(crate) unsafe fn clean_range(&self, range: Range<usize>) {
            let _ = self.vmo.op_range(zx::VmoOp::ZERO, range.start as u64, range.len() as u64);
        }
    }

    impl Drop for BufferSource {
        fn drop(&mut self) {
            // SAFETY: This balances the `map` in `new` above.
            unsafe {
                let _ = vmar_root_self().unmap(self.base as usize, self.size);
            }
        }
    }
}

#[cfg(not(target_os = "fuchsia"))]
mod buffer_source {
    use std::cell::UnsafeCell;
    use std::ops::Range;
    use std::pin::Pin;
    use storage_ptr_slice::MutPtrByteSlice;

    /// A basic heap-backed buffer source.
    #[derive(Debug)]
    pub struct BufferSource {
        // We use an UnsafeCell here because we need interior mutability of the buffer (to hand out
        // mutable slices to it in |buffer()|), but don't want to pay the cost of wrapping the
        // buffer in a Mutex. We must guarantee that the Buffer objects we hand out don't overlap,
        // but that is already a requirement for correctness.
        data: UnsafeCell<Pin<Vec<u8>>>,
    }

    // Safe because none of the fields in BufferSource are modified, except the contents of `data`,
    // but that is managed by the BufferAllocator.
    unsafe impl Sync for BufferSource {}

    impl BufferSource {
        pub fn new(size: usize) -> Self {
            Self { data: UnsafeCell::new(Pin::new(vec![0 as u8; size])) }
        }

        pub fn new_trusted(size: usize) -> Self {
            Self::new(size)
        }

        pub fn is_trusted(&self) -> bool {
            true
        }

        pub fn size(&self) -> usize {
            // Safe because the reference goes out of scope as soon as we use it.
            unsafe { (&*self.data.get()).len() }
        }

        /// Returns a mutable pointer slice for the given range.
        ///
        /// # Safety
        ///
        /// The caller must ensure that no other active references or pointer slices overlap with
        /// this range.
        pub(super) unsafe fn subslice_ptr(&self, range: &Range<usize>) -> MutPtrByteSlice<'_> {
            assert!(range.start < self.size() && range.end <= self.size());
            // SAFETY: The data vector is valid for `size()` bytes, and `range` is within bounds.
            // The caller guarantees exclusivity.
            unsafe {
                let ptr = (&mut *self.data.get()).as_mut_ptr().add(range.start);
                MutPtrByteSlice::new(std::ptr::slice_from_raw_parts_mut(ptr, range.len()))
            }
        }

        /// Returns a mutable pointer slice with an arbitrary lifetime `'a` for the given range.
        ///
        /// # Safety
        ///
        /// The caller must ensure that no other active references or pointer slices overlap with
        /// this range, and that `self` remains valid and mapped in memory for the entire lifetime
        /// `'a`.
        pub(super) unsafe fn subslice_ptr_unbounded<'a>(
            &self,
            range: &Range<usize>,
        ) -> MutPtrByteSlice<'a> {
            assert!(range.start < self.size() && range.end <= self.size());
            // SAFETY: The data vector is valid for `size()` bytes, and `range` is within bounds.
            // The caller guarantees exclusivity and memory liveness for `'a`.
            unsafe {
                let ptr = (&mut *self.data.get()).as_mut_ptr().add(range.start);
                MutPtrByteSlice::new(std::ptr::slice_from_raw_parts_mut(ptr, range.len()))
            }
        }

        /// Zeroes out the range.
        ///
        /// # Safety
        ///
        /// The range must not be allocated.
        pub(super) unsafe fn clean_range(&self, range: Range<usize>) {
            // SAFETY: The caller guarantees the range is not allocated.
            unsafe { self.subslice_ptr(&range) }.fill(0);
        }
    }
}

pub use buffer_source::BufferSource;

// Stores a list of offsets into a BufferSource. The size of the free ranges is determined by which
// FreeList we are looking at.
//
// FreeLists are kept sorted in descending order of offset (highest offset at the front, lowest
// offset at the back). This allows `pop()` to remove and return the lowest offset in O(1) time,
// ensuring the allocator allocates from lowest addresses to highest addresses (first-fit).
// This guarantees that allocations are packed into lower addresses, and that recently freed lower
// offsets are prioritized for reuse. `PinnedBufferAllocator` relies on this property to ensure
// allocations pack into the permanently pinned first chunk before spilling over into dynamically
// pinned chunks.
type FreeList = Vec<usize>;

#[derive(Debug)]
struct Inner {
    // The index corresponds to the order of free memory blocks in the free list.
    free_lists: Vec<FreeList>,
    // Maps offsets to allocated length (the actual length, not the size requested by the client).
    allocation_map: BTreeMap<usize, usize>,
}

/// BufferAllocator creates Buffer objects to be used for block device I/O requests.
///
/// This is implemented through a simple buddy allocation scheme. Allocations always prioritize
/// the lowest available offset (first-fit), packing allocations toward the beginning of the
/// memory pool and reusing recently freed lower offsets.
#[derive(Debug)]
pub struct BufferAllocator {
    block_size: usize,
    source: BufferSource,
    inner: Mutex<Inner>,
    event: Event,
}

// Returns the smallest order which is at least `size` bytes.
fn order(size: usize, block_size: usize) -> usize {
    if size <= block_size {
        return 0;
    }
    let nblocks = round_up(size, block_size) / block_size;
    nblocks.next_power_of_two().trailing_zeros() as usize
}

// Returns the largest order which is no more than `size` bytes.
fn order_fit(size: usize, block_size: usize) -> usize {
    assert!(size >= block_size);
    let nblocks = round_up(size, block_size) / block_size;
    if nblocks.is_power_of_two() {
        nblocks.trailing_zeros() as usize
    } else {
        nblocks.next_power_of_two().trailing_zeros() as usize - 1
    }
}

fn size_for_order(order: usize, block_size: usize) -> usize {
    block_size * (1 << (order as u32))
}

fn initial_free_lists(size: usize, block_size: usize) -> Vec<FreeList> {
    let size = round_down(size, block_size);
    assert!(block_size <= size);
    assert!(block_size.is_power_of_two());
    let max_order = order_fit(size, block_size);
    let mut free_lists = Vec::with_capacity(max_order + 1);
    for _ in 0..=max_order {
        free_lists.push(FreeList::new())
    }
    let mut offset = 0;
    while offset < size {
        let order = order_fit(size - offset, block_size);
        let size = size_for_order(order, block_size);
        free_lists[order].push(offset);
        offset += size;
    }
    free_lists
}

/// A trait for buffer allocators that support asynchronous buffer allocation via event listeners.
pub trait TryAllocateBuffer<'a> {
    type Buffer;

    /// Attempts to allocate a buffer of `size` bytes. Returns an [`EventListener`] to wait on if
    /// memory is temporarily unavailable.
    fn try_allocate_buffer(&'a self, size: usize) -> Result<Self::Buffer, EventListener>;

    /// Allocates a buffer synchronously, blocking the current thread until memory is available.
    fn allocate_buffer_sync(&'a self, size: usize) -> Self::Buffer {
        loop {
            match self.try_allocate_buffer(size) {
                Ok(buffer) => return buffer,
                Err(listener) => listener.wait(),
            }
        }
    }
}

/// A future which will resolve to an allocated buffer.
pub struct BufferFuture<'a, A: TryAllocateBuffer<'a> + ?Sized = BufferAllocator> {
    allocator: &'a A,
    size: usize,
    listener: Option<EventListener>,
}

impl<'a, A: TryAllocateBuffer<'a> + ?Sized> BufferFuture<'a, A> {
    pub fn new(allocator: &'a A, size: usize) -> Self {
        Self { allocator, size, listener: None }
    }
}

impl<'a, A: TryAllocateBuffer<'a> + ?Sized> Future for BufferFuture<'a, A> {
    type Output = A::Buffer;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(listener) = self.listener.as_mut() {
            futures::ready!(listener.poll_unpin(context));
        }
        // Loop because we need to deal with the case where `listener` is ready immediately upon
        // creation, in which case we ought to retry the allocation.
        loop {
            match self.allocator.try_allocate_buffer(self.size) {
                Ok(buffer) => return Poll::Ready(buffer),
                Err(mut listener) => {
                    if listener.poll_unpin(context).is_pending() {
                        self.listener = Some(listener);
                        return Poll::Pending;
                    }
                }
            }
        }
    }
}

impl BufferAllocator {
    pub fn new(block_size: usize, source: BufferSource) -> Self {
        let free_lists = initial_free_lists(source.size(), block_size);
        Self {
            block_size,
            source,
            inner: Mutex::new(Inner { free_lists, allocation_map: BTreeMap::new() }),
            event: Event::new(),
        }
    }

    /// Returns the underlying VMO if the allocator is untrusted.
    ///
    /// Returns `None` if `is_trusted()` is true, because exposing the VMO would allow
    /// external modification of memory that is assumed to be unshared.
    #[cfg(target_os = "fuchsia")]
    pub fn vmo(&self) -> Option<Arc<zx::Vmo>> {
        if self.is_trusted() { None } else { Some(self.source.vmo().clone()) }
    }

    pub fn is_trusted(&self) -> bool {
        self.source.is_trusted()
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn buffer_source(&self) -> &BufferSource {
        &self.source
    }

    /// Takes the buffer source from the allocator and consumes the allocator.
    pub fn take_buffer_source(self) -> BufferSource {
        self.source
    }

    /// Returns an identifier for this allocator based on its memory address.
    pub fn identifier(&self) -> usize {
        std::ptr::from_ref(self).addr()
    }

    /// Allocates a Buffer with capacity for `size` bytes. Blocks until there are enough bytes
    /// available to satisfy the request.
    ///
    /// The allocated buffer will be block-aligned and the padding up to block alignment can also
    /// be used by the buffer.
    ///
    /// Allocation is O(lg(N) + M), where N = size and M = number of allocations.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds the pool size (`self.buffer_source().size()`).
    pub fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        BufferFuture::new(self, size)
    }

    /// Allocates a Buffer with capacity for `size` bytes synchronously. Blocks the current thread
    /// until enough memory is available.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds the pool size (`self.buffer_source().size()`).
    pub fn allocate_buffer_sync(&self, size: usize) -> Buffer<'_> {
        <Self as TryAllocateBuffer>::allocate_buffer_sync(self, size)
    }

    /// Allocates an OwnedBuffer with capacity for `size` bytes synchronously. Blocks the current
    /// thread until enough memory is available.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds the pool size (`self.buffer_source().size()`).
    pub fn allocate_buffer_sync_owned(self: &Arc<Self>, size: usize) -> OwnedBuffer {
        loop {
            match self.try_allocate_buffer_owned(size) {
                Ok(buffer) => return buffer,
                Err(listener) => listener.wait(),
            }
        }
    }

    /// Allocates an OwnedBuffer non-blockingly, returning an EventListener if memory is
    /// unavailable.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds the pool size (`self.buffer_source().size()`).
    pub fn try_allocate_buffer_owned(
        self: &Arc<Self>,
        size: usize,
    ) -> Result<OwnedBuffer, EventListener> {
        let buffer = self.try_allocate_buffer(size)?;
        let range = buffer.range();
        // SAFETY: `try_allocate_buffer` guarantees that `range` does not overlap with any other
        // active allocations. We hold `Arc<Self>` (`self.clone()`), which guarantees that
        // `self.source` remains valid and mapped in memory for the entire `'static` existence of
        // `OwnedBuffer`.
        let slice = unsafe { self.source.subslice_ptr_unbounded(&range) };
        std::mem::forget(buffer);
        Ok(OwnedBuffer::new(slice, range, self.clone() as Arc<dyn BufferAllocatorTrait>))
    }

    /// Like `allocate_buffer`, but returns an EventListener if the allocation cannot be satisfied.
    /// The listener will signal when the caller should try again.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds the pool size (`self.buffer_source().size()`).
    pub fn try_allocate_buffer(&self, size: usize) -> Result<Buffer<'_>, EventListener> {
        if size > self.source.size() {
            panic!("Allocation of {} bytes would exceed limit {}", size, self.source.size());
        }
        let mut inner = self.inner.lock();
        let requested_order = order(size, self.block_size());
        assert!(requested_order < inner.free_lists.len());
        // Pick the smallest possible order with a free entry.
        let mut order = {
            let mut idx = requested_order;
            loop {
                if idx >= inner.free_lists.len() {
                    return Err(self.event.listen());
                }
                if !inner.free_lists[idx].is_empty() {
                    break idx;
                }
                idx += 1;
            }
        };

        // Split the free region until it's the right size.
        let offset = inner.free_lists[order].pop().unwrap();
        while order > requested_order {
            order -= 1;
            assert!(inner.free_lists[order].is_empty());
            inner.free_lists[order].push(offset + self.size_for_order(order));
        }

        inner.allocation_map.insert(offset, self.size_for_order(order));
        let range = offset..offset + size;
        log::debug!(range:?, bytes_used = self.size_for_order(order); "Allocated");

        // SAFETY: The allocator guarantees that this range does not overlap with any other
        // active allocations. `self` guarantees `self.source` remains valid for `'a`.
        Ok(Buffer::new(unsafe { self.source.subslice_ptr(&range) }, range, self))
    }
}

impl BufferAllocatorTrait for BufferAllocator {
    fn free_buffer(&self, range: Range<usize>) {
        self.free_buffer(range);
    }

    fn is_trusted(&self) -> bool {
        self.is_trusted()
    }

    #[cfg(target_os = "fuchsia")]
    fn vmo(&self) -> Option<Arc<zx::Vmo>> {
        self.vmo()
    }
}

impl<'a> TryAllocateBuffer<'a> for BufferAllocator {
    type Buffer = Buffer<'a>;

    fn try_allocate_buffer(&'a self, size: usize) -> Result<Buffer<'a>, EventListener> {
        self.try_allocate_buffer(size)
    }
}

impl BufferAllocator {
    /// Deallocation is O(lg(N) + M), where N = size and M = number of allocations.
    #[doc(hidden)]
    pub(crate) fn free_buffer(&self, range: Range<usize>) {
        let mut inner = self.inner.lock();
        let mut offset = range.start;
        let size = inner
            .allocation_map
            .remove(&offset)
            .unwrap_or_else(|| panic!("No allocation record found for {:?}", range));
        assert!(range.end - range.start <= size);
        log::debug!(range:?, bytes_used = size; "Freeing");

        // Merge as many free slots as we can.
        let mut order = order(size, self.block_size());
        while order < inner.free_lists.len() - 1 {
            let buddy = self.find_buddy(offset, order);
            let idx = if let Ok(idx) =
                inner.free_lists[order].binary_search_by(|probe| buddy.cmp(probe))
            {
                idx
            } else {
                break;
            };
            inner.free_lists[order].remove(idx);
            offset = std::cmp::min(offset, buddy);
            order += 1;
        }

        let idx = match inner.free_lists[order].binary_search_by(|probe| offset.cmp(probe)) {
            Ok(_) => panic!("Unexpectedly found {} in free list {}", offset, order),
            Err(idx) => idx,
        };
        inner.free_lists[order].insert(idx, offset);

        // Notify all stuck tasks.  This might be inefficient, but it's simple and correct.
        self.event.notify(usize::MAX);
    }

    fn size_for_order(&self, order: usize) -> usize {
        size_for_order(order, self.block_size)
    }

    fn find_buddy(&self, offset: usize, order: usize) -> usize {
        offset ^ self.size_for_order(order)
    }

    /// Zeroes out all unnallocated ranges in the transfer buffer.
    pub fn clean_transfer_buffer(&self) {
        let inner = self.inner.lock();
        for (n, free_list) in inner.free_lists.iter().enumerate() {
            for offset in free_list {
                // SAFETY: The range is not allocated.
                unsafe {
                    self.source.clean_range(*offset..(*offset + size_for_order(n, self.block_size)))
                };
            }
        }
    }
}

#[cfg(target_os = "fuchsia")]
pub use crate::pinned_buffer_allocator::{
    DEFAULT_PIN_CHUNK_SIZE, PinnedBuffer, PinnedBufferAllocator, PinnedBufferFuture,
};

#[cfg(test)]
mod tests {
    use crate::buffer::BufferAllocator as BufferAllocatorTrait;
    use crate::buffer_allocator::{BufferAllocator, BufferSource, order};
    use fuchsia_async as fasync;
    use futures::future::join_all;
    use futures::pin_mut;
    use rand::seq::IndexedRandom as _;
    use rand::{RngExt as _, rng};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[fuchsia::test]
    async fn test_odd_sized_buffer_source() {
        let source = BufferSource::new(123);
        let allocator = BufferAllocator::new(2, source);

        // 123 == 64 + 32 + 16 + 8 + 2 + 1. (The last byte is unusable.)
        let sizes = vec![64, 32, 16, 8, 2];
        let mut bufs = vec![];
        for size in sizes.iter() {
            bufs.push(allocator.allocate_buffer(*size).await);
        }
        for (expected_size, buf) in sizes.iter().zip(bufs.iter()) {
            assert_eq!(*expected_size, buf.len());
        }
        assert!(allocator.try_allocate_buffer(2).is_err());
    }

    #[fuchsia::test]
    async fn test_allocate_buffer_read_write() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(8192, source);

        let mut buf = allocator.allocate_buffer(8192).await;
        buf.fill(0xaa);
        let mut vec = vec![0 as u8; 8192];
        buf.copy_to_slice(&mut vec);
        assert_eq!(vec, vec![0xaa as u8; 8192]);
    }

    #[fuchsia::test]
    async fn test_allocate_buffer_consecutive_calls_do_not_overlap() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(8192, source);

        let buf1 = allocator.allocate_buffer(8192).await;
        let buf2 = allocator.allocate_buffer(8192).await;
        assert!(buf1.range().end <= buf2.range().start || buf2.range().end <= buf1.range().start);
    }

    #[fuchsia::test]
    async fn test_allocate_many_buffers() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(8192, source);

        for _ in 0..10 {
            let _ = allocator.allocate_buffer(8192).await;
        }
    }

    #[fuchsia::test]
    async fn test_allocate_small_buffers_dont_overlap() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(8192, source);

        let buf1 = allocator.allocate_buffer(1).await;
        let buf2 = allocator.allocate_buffer(1).await;
        assert!(buf1.range().end <= buf2.range().start || buf2.range().end <= buf1.range().start);
    }

    #[fuchsia::test]
    async fn test_allocate_large_buffer() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(8192, source);

        let mut buf = allocator.allocate_buffer(1024 * 1024).await;
        assert_eq!(buf.len(), 1024 * 1024);
        buf.fill(0xaa);
        let mut vec = vec![0 as u8; 1024 * 1024];
        buf.copy_to_slice(&mut vec);
        assert_eq!(vec, vec![0xaa as u8; 1024 * 1024]);
    }

    #[fuchsia::test]
    async fn test_allocate_large_buffer_after_smaller_buffers() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(8192, source);

        {
            let mut buffers = vec![];
            while let Ok(buffer) = allocator.try_allocate_buffer(8192) {
                buffers.push(buffer);
            }
        }
        let buf = allocator.allocate_buffer(1024 * 1024).await;
        assert_eq!(buf.len(), 1024 * 1024);
    }

    #[fuchsia::test]
    async fn test_allocate_at_limits() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(8192, source);

        let mut buffers = vec![];
        while let Ok(buffer) = allocator.try_allocate_buffer(8192) {
            buffers.push(buffer);
        }
        // Deallocate a single buffer, and reallocate a single one back.
        buffers.pop();
        let buf = allocator.allocate_buffer(8192).await;
        assert_eq!(buf.len(), 8192);
    }

    #[fuchsia::test(threads = 10)]
    async fn test_random_allocs_deallocs() {
        let source = BufferSource::new(16 * 1024 * 1024);
        let bs = 512;
        let allocator = Arc::new(BufferAllocator::new(bs, source));

        join_all((0..10).map(|_| {
            let allocator = allocator.clone();
            fasync::Task::spawn(async move {
                let mut rng = rng();
                enum Op {
                    Alloc,
                    Dealloc,
                }
                let ops = vec![Op::Alloc, Op::Dealloc];
                let mut buffers = vec![];
                for _ in 0..1000 {
                    match ops.choose(&mut rng).unwrap() {
                        Op::Alloc => {
                            // Rather than a uniform distribution 1..64K, first pick an order and
                            // then pick a size within that. For example, we might pick order 3,
                            // which would give us 8 * 512..16 * 512 as our possible range.
                            // This way we don't bias towards larger allocations too much.
                            let order: usize = rng.random_range(order(1, bs)..order(65536 + 1, bs));
                            let size: usize = rng.random_range(
                                bs * 2_usize.pow(order as u32)..bs * 2_usize.pow(order as u32 + 1),
                            );
                            if let Ok(mut buf) = allocator.try_allocate_buffer(size) {
                                let val = rng.random::<u8>();
                                buf.fill(val);
                                let mut data = vec![0u8; size];
                                buf.copy_to_slice(&mut data);
                                for v in &data {
                                    assert_eq!(v, &val);
                                }
                                buffers.push(buf);
                            }
                        }
                        Op::Dealloc if !buffers.is_empty() => {
                            let idx = rng.random_range(0..buffers.len());
                            buffers.remove(idx);
                        }
                        _ => {}
                    };
                }
            })
        }))
        .await;
    }

    #[fuchsia::test]
    async fn test_buffer_refs() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(512, source);

        // Allocate one buffer first so that `buf` is not starting at offset 0. This helps catch
        // bugs.
        let _buf = allocator.allocate_buffer(512).await;
        let mut buf = allocator.allocate_buffer(4096).await;
        let base = buf.range().start;
        {
            let mut bref = buf.subslice_mut(1000..2000);
            assert_eq!(bref.len(), 1000);
            assert_eq!(bref.range(), base + 1000..base + 2000);
            bref.fill(0xbb);
            {
                let mut bref2 = bref.reborrow().subslice_mut(0..100);
                assert_eq!(bref2.len(), 100);
                assert_eq!(bref2.range(), base + 1000..base + 1100);
                bref2.fill(0xaa);
            }
            {
                let mut bref2 = bref.reborrow().subslice_mut(900..1000);
                assert_eq!(bref2.len(), 100);
                assert_eq!(bref2.range(), base + 1900..base + 2000);
                bref2.fill(0xcc);
            }
            let mut data = vec![0u8; 1000];
            bref.copy_to_slice(&mut data);
            assert_eq!(data[..100], vec![0xaa; 100]);
            assert_eq!(data[100..900], vec![0xbb; 800]);

            let bref = bref.subslice_mut(900..);
            assert_eq!(bref.len(), 100);
            let mut data = vec![0u8; 100];
            bref.copy_to_slice(&mut data);
            assert_eq!(data, vec![0xcc; 100]);
        }
        {
            let bref = buf.as_ref();
            assert_eq!(bref.len(), 4096);
            assert_eq!(bref.range(), base..base + 4096);
            let mut data = vec![0u8; 4096];
            bref.copy_to_slice(&mut data);
            assert_eq!(data[0..1000], vec![0x00; 1000]);
            {
                let bref2 = bref.subslice(1000..2000);
                assert_eq!(bref2.len(), 1000);
                assert_eq!(bref2.range(), base + 1000..base + 2000);
                let mut data2 = vec![0u8; 1000];
                bref2.copy_to_slice(&mut data2);
                assert_eq!(data2[..100], vec![0xaa; 100]);
                assert_eq!(data2[100..900], vec![0xbb; 800]);
                assert_eq!(data2[900..1000], vec![0xcc; 100]);
            }

            let bref = bref.subslice(2048..);
            assert_eq!(bref.len(), 2048);
            let mut data = vec![0u8; 2048];
            bref.copy_to_slice(&mut data);
            assert_eq!(data, vec![0x00; 2048]);
        }
    }

    #[fuchsia::test]
    async fn test_buffer_split() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = BufferAllocator::new(512, source);

        // Allocate one buffer first so that `buf` is not starting at offset 0. This helps catch
        // bugs.
        let _buf = allocator.allocate_buffer(512).await;
        let mut buf = allocator.allocate_buffer(4096).await;
        let base = buf.range().start;
        {
            let bref = buf.as_mut();
            let (mut s1, mut s2) = bref.split_at_mut(2048);
            assert_eq!(s1.len(), 2048);
            assert_eq!(s1.range(), base..base + 2048);
            s1.fill(0xaa);
            assert_eq!(s2.len(), 2048);
            assert_eq!(s2.range(), base + 2048..base + 4096);
            s2.fill(0xbb);
        }
        {
            let bref = buf.as_ref();
            let (s1, s2) = bref.split_at(1);
            let (s2, s3) = s2.split_at(2047);
            let (s3, s4) = s3.split_at(0);
            assert_eq!(s1.len(), 1);
            assert_eq!(s1.range(), base..base + 1);
            assert_eq!(s2.len(), 2047);
            assert_eq!(s2.range(), base + 1..base + 2048);
            assert_eq!(s3.len(), 0);
            assert_eq!(s3.range(), base + 2048..base + 2048);
            assert_eq!(s4.len(), 2048);
            assert_eq!(s4.range(), base + 2048..base + 4096);
            let mut d1 = vec![0u8; 1];
            s1.copy_to_slice(&mut d1);
            assert_eq!(d1, vec![0xaa; 1]);
            let mut d2 = vec![0u8; 2047];
            s2.copy_to_slice(&mut d2);
            assert_eq!(d2, vec![0xaa; 2047]);
            let mut d3 = vec![0u8; 0];
            s3.copy_to_slice(&mut d3);
            assert_eq!(d3, &[] as &[u8]);
            let mut d4 = vec![0u8; 2048];
            s4.copy_to_slice(&mut d4);
            assert_eq!(d4, vec![0xbb; 2048]);
        }
    }

    #[fuchsia::test]
    async fn test_blocking_allocation() {
        let source = BufferSource::new(1024 * 1024);
        let allocator = Arc::new(BufferAllocator::new(512, source));

        let buf1 = allocator.allocate_buffer(512 * 1024).await;
        let buf2 = allocator.allocate_buffer(512 * 1024).await;
        let bufs_dropped = Arc::new(AtomicBool::new(false));

        // buf3_fut should block until both buf1 and buf2 are done.
        let allocator_clone = allocator.clone();
        let bufs_dropped_clone = bufs_dropped.clone();
        let buf3_fut = async move {
            allocator_clone.allocate_buffer(1024 * 1024).await;
            assert!(bufs_dropped_clone.load(Ordering::Relaxed), "Allocation finished early");
        };
        pin_mut!(buf3_fut);

        // Each of buf_futs should block until buf3_fut is done, and they should proceed in order.
        let mut buf_futs = vec![];
        for _ in 0..16 {
            let allocator_clone = allocator.clone();
            let bufs_dropped_clone = bufs_dropped.clone();
            let fut = async move {
                allocator_clone.allocate_buffer(64 * 1024).await;
                // We can't say with certainty that buf3 proceeded first, nor can we ensure these
                // allocations proceed in order, but we can make sure that at least buf1/buf2 were
                // done (since they exhausted the pool).
                assert!(bufs_dropped_clone.load(Ordering::Relaxed), "Allocation finished early");
            };
            buf_futs.push(fut);
        }

        futures::join!(buf3_fut, join_all(buf_futs), async move {
            std::mem::drop(buf1);
            std::mem::drop(buf2);
            bufs_dropped.store(true, Ordering::Relaxed);
        });
    }

    #[fuchsia::test]
    async fn test_clean_entire_transfer_buffer() {
        const BUFFER_SIZE: usize = 4096;
        let source = BufferSource::new(BUFFER_SIZE);
        let allocator = Arc::new(BufferAllocator::new(512, source));

        let mut buf = allocator.allocate_buffer(BUFFER_SIZE).await;
        buf.fill(0xaa);
        std::mem::drop(buf);

        allocator.clean_transfer_buffer();
        let buf = allocator.allocate_buffer(BUFFER_SIZE).await;
        let mut data = vec![0u8; BUFFER_SIZE];
        buf.copy_to_slice(&mut data);
        assert_eq!(data, vec![0; BUFFER_SIZE]);
    }

    #[fuchsia::test]
    async fn test_clean_transfer_buffer_around_allocation() {
        let source = BufferSource::new(4096);
        let allocator = Arc::new(BufferAllocator::new(512, source));

        let mut buf1 = allocator.allocate_buffer(1024).await;
        buf1.fill(0xaa);
        let mut buf2 = allocator.allocate_buffer(1024).await;
        buf2.fill(0xbb);
        assert_eq!(buf2.range().start, 1024);
        let mut buf3 = allocator.allocate_buffer(2048).await;
        buf3.fill(0xcc);
        std::mem::drop(buf1);
        std::mem::drop(buf3);

        allocator.clean_transfer_buffer();

        let buf1 = allocator.allocate_buffer(1024).await;
        let mut data1 = vec![0u8; 1024];
        buf1.copy_to_slice(&mut data1);
        assert_eq!(data1, vec![0; 1024]);

        let mut data2 = vec![0u8; 1024];
        buf2.copy_to_slice(&mut data2);
        assert_eq!(data2, vec![0xbb; 1024]);

        let buf3 = allocator.allocate_buffer(2048).await;
        let mut data3 = vec![0u8; 2048];
        buf3.copy_to_slice(&mut data3);
        assert_eq!(data3, vec![0; 2048]);
    }

    #[fuchsia::test]
    async fn test_safe_buffer_apis() {
        let source = BufferSource::new(4096);
        let allocator = BufferAllocator::new(512, source);

        let mut buf = allocator.allocate_buffer(4096).await;

        // Test copy_from_slice and copy_to_slice
        let input_data = vec![0x33_u8; 4096];
        buf.copy_from_slice(&input_data);
        let mut output_data = vec![0_u8; 4096];
        buf.copy_to_slice(&mut output_data);
        assert_eq!(input_data, output_data);

        // Test fill
        buf.fill(0x55);
        buf.copy_to_slice(&mut output_data);
        assert_eq!(output_data, vec![0x55; 4096]);

        // Test subslice
        {
            let mut sub_mut = buf.as_mut().subslice_mut(1024..2048);
            assert_eq!(sub_mut.len(), 1024);
            sub_mut.fill(0xaa);
        }

        {
            let bref = buf.as_ref();
            let sub_ref = bref.subslice(1024..2048);
            assert_eq!(sub_ref.len(), 1024);
            let mut sub_output = vec![0_u8; 1024];
            sub_ref.copy_to_slice(&mut sub_output);
            assert_eq!(sub_output, vec![0xaa; 1024]);
        }

        // Test split_at and split_at_mut
        {
            let (mut left_mut, mut right_mut) = buf.as_mut().split_at_mut(2048);
            assert_eq!(left_mut.len(), 2048);
            assert_eq!(right_mut.len(), 2048);
            left_mut.fill(0x11);
            right_mut.fill(0x22);
        }

        {
            let bref = buf.as_ref();
            let (left_ref, right_ref) = bref.split_at(2048);
            let mut left_out = vec![0_u8; 2048];
            let mut right_out = vec![0_u8; 2048];
            left_ref.copy_to_slice(&mut left_out);
            right_ref.copy_to_slice(&mut right_out);
            assert_eq!(left_out, vec![0x11; 2048]);
            assert_eq!(right_out, vec![0x22; 2048]);
        }
    }

    #[fuchsia::test]
    async fn test_owned_buffer() {
        let source = BufferSource::new(4096);
        let allocator = Arc::new(BufferAllocator::new(512, source));

        let mut owned_buf = allocator.allocate_buffer_sync_owned(2048);
        assert_eq!(owned_buf.len(), 2048);
        owned_buf.as_mut_ptr_slice().fill(0xcc);
        assert_eq!(owned_buf.as_ptr_slice().to_vec(), vec![0xcc; 2048]);

        // Allocating remaining 2048 bytes should succeed.
        let owned_buf2 = allocator.try_allocate_buffer_owned(2048).expect("Must succeed");
        assert_eq!(owned_buf2.len(), 2048);

        // Pool is full (4096 bytes used). Next allocation should return an EventListener.
        assert!(allocator.try_allocate_buffer_owned(512).is_err());

        // Dropping owned_buf should free its 2048 bytes back to the allocator.
        std::mem::drop(owned_buf);

        // Now allocation of 2048 bytes should succeed again.
        let mut owned_buf3 = allocator.try_allocate_buffer_owned(2048).expect("Must succeed");
        owned_buf3.as_mut_ptr_slice().fill(0xdd);
        assert_eq!(owned_buf3.as_ptr_slice().to_vec(), vec![0xdd; 2048]);
    }

    #[fuchsia::test]
    async fn test_allocate_buffer_sync() {
        let source = BufferSource::new(4096);
        let allocator = BufferAllocator::new(512, source);

        let mut buf = allocator.allocate_buffer_sync(2048);
        assert_eq!(buf.len(), 2048);
        buf.as_mut_ptr_slice().fill(0xee);
        assert_eq!(buf.as_ptr_slice().to_vec(), vec![0xee; 2048]);

        std::mem::drop(buf);

        let mut buf2 = allocator.allocate_buffer_sync(4096);
        assert_eq!(buf2.len(), 4096);
        buf2.as_mut_ptr_slice().fill(0xff);
        assert_eq!(buf2.as_ptr_slice().to_vec(), vec![0xff; 4096]);
    }

    #[fuchsia::test]
    async fn test_trusted_buffer_apis() {
        // Untrusted allocator (only relevant/testable on Fuchsia)
        #[cfg(target_os = "fuchsia")]
        {
            let source = BufferSource::new(4096);
            let allocator = BufferAllocator::new(512, source);
            let mut buf = allocator.allocate_buffer(4096).await;
            assert!(buf.try_as_slice().is_none());
            assert!(buf.as_mut().try_as_mut_slice().is_none());
        }

        // Trusted allocator (with trusted source)
        {
            let source = BufferSource::new_trusted(4096);
            let allocator = BufferAllocator::new(512, source);
            let mut buf = allocator.allocate_buffer(4096).await;
            assert!(buf.try_as_slice().is_some());
            assert!(buf.as_mut().try_as_mut_slice().is_some());

            // Verify we can actually read/write via slice
            let slice = buf.try_as_slice().unwrap();
            assert_eq!(slice.len(), 4096);
            let mut expected = vec![0u8; 4096];
            assert_eq!(slice, expected.as_slice());

            let mut bref = buf.as_mut();
            let slice_mut = bref.try_as_mut_slice().unwrap();
            slice_mut[0] = 0xff;
            expected[0] = 0xff;
            assert_eq!(buf.try_as_slice().unwrap(), expected.as_slice());
        }
    }

    #[fuchsia::test]
    #[cfg(target_os = "fuchsia")]
    async fn test_trusted_buffer_rights() {
        use zx;
        let source = BufferSource::new_trusted(4096);
        let vmo = source.vmo();
        let info = vmo.basic_info().expect("failed to get basic info");
        assert!(!info.rights.contains(zx::Rights::TRANSFER));
    }

    #[fuchsia::test]
    async fn test_allocator_lowest_offset_first() {
        let source = BufferSource::new(8192);
        let allocator = BufferAllocator::new(512, source);

        // Allocate two 1024-byte buffers.
        let buf0 = allocator.allocate_buffer(1024).await;
        let buf1 = allocator.allocate_buffer(1024).await;
        assert_eq!(buf0.range(), 0..1024);
        assert_eq!(buf1.range(), 1024..2048);

        // Drop buf0. The lower offset (0) is freed.
        std::mem::drop(buf0);

        // Next allocation should prioritize the lowest available offset (0), not 2048.
        let buf0_again = allocator.allocate_buffer(1024).await;
        assert_eq!(buf0_again.range(), 0..1024);
    }

    #[fuchsia::test]
    #[cfg(target_os = "fuchsia")]
    async fn test_unpinned_buffer() {
        let source = BufferSource::new(4096);
        let standard_allocator = BufferAllocator::new(512, source);
        let unpinned = standard_allocator.allocate_buffer(512).await;
        assert_eq!(unpinned.paddrs(), None);
        assert_eq!(unpinned.contiguity(), None);
    }

    #[fuchsia::test]
    #[cfg(target_os = "fuchsia")]
    async fn test_vmo_trusted_vs_untrusted() {
        let untrusted_source = BufferSource::new(4096);
        let untrusted_alloc = BufferAllocator::new(512, untrusted_source);
        assert!(!untrusted_alloc.is_trusted());
        assert!(untrusted_alloc.vmo().is_some());
        assert!(BufferAllocatorTrait::vmo(&untrusted_alloc).is_some());
        let untrusted_buf = untrusted_alloc.allocate_buffer(512).await;
        assert!(untrusted_buf.vmo().is_some());

        let trusted_source = BufferSource::new_trusted(4096);
        let trusted_alloc = BufferAllocator::new(512, trusted_source);
        assert!(trusted_alloc.is_trusted());
        assert!(trusted_alloc.vmo().is_none());
        assert!(BufferAllocatorTrait::vmo(&trusted_alloc).is_none());
        let trusted_buf = trusted_alloc.allocate_buffer(512).await;
        assert!(trusted_buf.vmo().is_none());
    }
}
