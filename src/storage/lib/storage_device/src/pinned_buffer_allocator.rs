// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffer::{BufferAllocator as BufferAllocatorTrait, BufferImpl, OwnedBuffer};
use crate::buffer_allocator::{BufferAllocator, BufferFuture, BufferSource, TryAllocateBuffer};
use event_listener::{EventListener, Listener as _};
use fuchsia_sync::Mutex;
use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::ops::Range;
use std::sync::Arc;
use zx::sys::zx_paddr_t;

/// Default chunk size for pinning DMA buffers (1 MiB).
pub const DEFAULT_PIN_CHUNK_SIZE: usize = 1024 * 1024;

#[derive(Debug)]
struct Chunk {
    pmt: Option<zx::Pmt>,
    // Number of active buffer allocations spanning this chunk.
    ref_count: usize,
}

#[derive(Debug)]
struct PinnedInner {
    chunks: Vec<Chunk>,
}

/// A specialized buffer allocator that pins memory in coarse chunks (e.g. 1 MiB) for DMA.
///
/// The first chunk (chunk 0) is pinned at initialization and remains pinned for the lifetime of
/// the allocator to eliminate pinning latency on requests. Additional chunks are pinned
/// dynamically on demand when buffers spill over into them, and unpinned when all buffers within
/// those chunks are dropped.
///
/// This allocator relies on [`BufferAllocator`]'s lowest-offset-first allocation strategy so that
/// allocations pack into the permanently pinned first chunk before spilling over into dynamically
/// pinned chunks.
pub struct PinnedBufferAllocator {
    allocator: BufferAllocator,
    bti: zx::Bti,
    contiguity: u64,
    chunk_size: usize,
    inner: Mutex<PinnedInner>,
    paddrs: Box<[UnsafeCell<zx_paddr_t>]>,
}

// SAFETY: Synchronization of writes to `paddrs` is guarded by `inner: Mutex<PinnedInner>`:
// - A chunk's entries in `paddrs` are only written when `pmt.is_none()` under the lock.
// - While `pmt.is_some()`, the chunk's entries are immutable and never modified or unpinned.
// - Readers calling `paddrs()` hold a `Buffer` whose range is in that chunk, guaranteeing
//   `pmt.is_some()` and exclusive immutable access to those entries for the buffer's lifetime.
unsafe impl Send for PinnedBufferAllocator {}
unsafe impl Sync for PinnedBufferAllocator {}

impl Debug for PinnedBufferAllocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedBufferAllocator")
            .field("block_size", &self.block_size())
            .field("chunk_size", &self.chunk_size)
            .field("contiguity", &self.contiguity)
            .finish_non_exhaustive()
    }
}

pub type PinnedBuffer<'a> = BufferImpl<'a, &'a PinnedBufferAllocator, PinnedBufferAllocator>;

pub type PinnedBufferFuture<'a> = BufferFuture<'a, PinnedBufferAllocator>;

impl<'a> TryAllocateBuffer<'a> for PinnedBufferAllocator {
    type Buffer = PinnedBuffer<'a>;

    fn try_allocate_buffer(&'a self, size: usize) -> Result<PinnedBuffer<'a>, EventListener> {
        self.try_allocate_buffer(size)
    }
}

impl PinnedBufferAllocator {
    /// Creates a new `PinnedBufferAllocator` with default chunk size (1 MiB).
    ///
    /// `contiguity` specifies the minimum physical address contiguity (in bytes) required by the
    /// hardware/BTI, which determines the granularity of entries in [`paddrs`](Self::paddrs).
    pub fn new(block_size: usize, source: BufferSource, bti: zx::Bti, contiguity: u64) -> Self {
        Self::with_chunk_size(block_size, source, bti, contiguity, DEFAULT_PIN_CHUNK_SIZE)
    }

    /// Creates a new `PinnedBufferAllocator` with a custom `chunk_size`.
    ///
    /// `contiguity` specifies the minimum physical address contiguity (in bytes) required by the
    /// hardware/BTI, which determines the granularity of entries in [`paddrs`](Self::paddrs).
    pub fn with_chunk_size(
        block_size: usize,
        source: BufferSource,
        bti: zx::Bti,
        contiguity: u64,
        chunk_size: usize,
    ) -> Self {
        assert!(!source.is_trusted(), "PinnedBufferAllocator cannot use a trusted buffer source");
        assert!(chunk_size.is_power_of_two());
        assert!(chunk_size >= contiguity as usize);
        assert!(chunk_size % contiguity as usize == 0);
        let num_chunks = source.size().div_ceil(chunk_size);
        let total_paddrs = source.size().div_ceil(contiguity as usize);
        let mut paddrs = Vec::with_capacity(total_paddrs);
        for _ in 0..total_paddrs {
            paddrs.push(UnsafeCell::new(0));
        }
        let mut chunks = Vec::with_capacity(num_chunks);
        for _ in 0..num_chunks {
            chunks.push(Chunk { pmt: None, ref_count: 0 });
        }
        let allocator = BufferAllocator::new(block_size, source);
        let this = Self {
            allocator,
            bti,
            contiguity,
            chunk_size,
            inner: Mutex::new(PinnedInner { chunks }),
            paddrs: paddrs.into_boxed_slice(),
        };
        if num_chunks > 0 {
            let mut inner = this.inner.lock();
            this.pin_chunk_locked(&mut inner, 0);
        }
        this
    }

    pub fn block_size(&self) -> usize {
        self.allocator.block_size()
    }

    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    pub fn contiguity(&self) -> u64 {
        self.contiguity
    }

    pub fn buffer_source(&self) -> &BufferSource {
        self.allocator.buffer_source()
    }

    /// Returns the underlying VMO for DMA operations.
    pub fn vmo(&self) -> Option<Arc<zx::Vmo>> {
        self.allocator.vmo()
    }

    pub fn is_trusted(&self) -> bool {
        false
    }

    /// Decommits unallocated pages in the buffer so the kernel can reclaim memory.
    pub fn clean_transfer_buffer(&self) {
        self.allocator.clean_transfer_buffer();
    }

    fn pin_chunk_locked(&self, inner: &mut PinnedInner, chunk_idx: usize) {
        let chunk = &mut inner.chunks[chunk_idx];
        if chunk.pmt.is_some() {
            return;
        }
        let chunk_offset = chunk_idx * self.chunk_size;
        let chunk_len =
            std::cmp::min(self.chunk_size, self.allocator.buffer_source().size() - chunk_offset);
        let contiguity_usize = self.contiguity as usize;
        let paddr_start = chunk_offset / contiguity_usize;
        let num_paddrs = chunk_len.div_ceil(contiguity_usize);
        // SAFETY: We hold `inner` lock and `chunk.pmt.is_none()`, so no other thread is
        // reading or writing to this slice of `paddrs`.
        let paddr_slice = unsafe {
            let ptr = self.paddrs[paddr_start].get();
            std::slice::from_raw_parts_mut(ptr, num_paddrs)
        };
        let options =
            zx::BtiOptions::PERM_READ | zx::BtiOptions::PERM_WRITE | zx::BtiOptions::COMPRESS;
        let vmo = self.allocator.vmo().expect("PinnedBufferAllocator requires an untrusted VMO");
        let pmt = self
            .bti
            .pin(options, &vmo, chunk_offset as u64, chunk_len as u64, paddr_slice)
            .unwrap_or_else(|status| {
                panic!("Failed to pin chunk {chunk_idx}: {status:?}");
            });
        chunk.pmt = Some(pmt);
    }

    fn pin_range(&self, range: &Range<usize>) {
        let start_chunk = range.start / self.chunk_size;
        let end_chunk = (range.end + self.chunk_size - 1) / self.chunk_size;
        let mut inner = self.inner.lock();
        for chunk_idx in start_chunk..end_chunk {
            self.pin_chunk_locked(&mut inner, chunk_idx);
            inner.chunks[chunk_idx].ref_count += 1;
        }
    }

    pub(crate) fn free_buffer(&self, range: Range<usize>) {
        let start_chunk = range.start / self.chunk_size;
        let end_chunk = (range.end + self.chunk_size - 1) / self.chunk_size;
        let mut inner = self.inner.lock();
        for chunk_idx in start_chunk..end_chunk {
            let chunk = &mut inner.chunks[chunk_idx];
            assert!(chunk.ref_count > 0);
            chunk.ref_count -= 1;
            // The first chunk (chunk 0) remains pinned for the lifetime of the allocator.
            if chunk.ref_count == 0 && chunk_idx > 0 {
                if let Some(pmt) = chunk.pmt.take() {
                    // SAFETY: All buffers allocated in this chunk have been dropped.
                    let _ = unsafe { pmt.unpin() };
                }
                let chunk_offset = chunk_idx * self.chunk_size;
                let chunk_len = std::cmp::min(
                    self.chunk_size,
                    self.allocator.buffer_source().size() - chunk_offset,
                );
                // Zero/decommit the unpinned chunk so the kernel reclaims physical memory.
                unsafe {
                    self.allocator
                        .buffer_source()
                        .clean_range(chunk_offset..chunk_offset + chunk_len);
                }
            }
        }
        drop(inner);
        self.allocator.free_buffer(range);
    }

    /// Returns a slice of physical addresses covering `range` and the contiguity granularity (in
    /// bytes) represented by each address in the slice.
    pub fn paddrs(&self, range: &Range<usize>) -> Option<(&[zx_paddr_t], u64)> {
        let contiguity_usize = self.contiguity as usize;
        let start_page = range.start / contiguity_usize;
        let end_page = (range.end + contiguity_usize - 1) / contiguity_usize;
        let num_paddrs = end_page - start_page;
        // SAFETY: The caller holds an active Buffer for `range`, which guarantees that the
        // chunks covering `range` have `ref_count > 0` (or `chunk_idx == 0`)
        // and are pinned with immutable paddrs.
        let slice = unsafe {
            let ptr = self.paddrs[start_page].get();
            std::slice::from_raw_parts(ptr, num_paddrs)
        };
        Some((slice, self.contiguity))
    }

    pub fn try_allocate_buffer(&self, size: usize) -> Result<PinnedBuffer<'_>, EventListener> {
        let buffer = self.allocator.try_allocate_buffer(size)?;
        let range = buffer.range();
        self.pin_range(&range);
        let slice = unsafe { self.allocator.buffer_source().subslice_ptr(&range) };
        // Forget the inner buffer so its Drop doesn't free the allocation prematurely;
        // ownership is transferred to the returned PinnedBuffer.
        std::mem::forget(buffer);
        Ok(BufferImpl::new(slice, range, self))
    }

    pub fn allocate_buffer_sync(&self, size: usize) -> PinnedBuffer<'_> {
        <Self as TryAllocateBuffer>::allocate_buffer_sync(self, size)
    }

    pub fn allocate_buffer(&self, size: usize) -> PinnedBufferFuture<'_> {
        BufferFuture::new(self, size)
    }

    pub fn try_allocate_buffer_owned(
        self: &Arc<Self>,
        size: usize,
    ) -> Result<OwnedBuffer, EventListener> {
        let buffer = self.allocator.try_allocate_buffer(size)?;
        let range = buffer.range();
        self.pin_range(&range);
        let slice = unsafe { self.allocator.buffer_source().subslice_ptr_unbounded(&range) };
        // Forget the inner buffer so its Drop doesn't free the allocation prematurely;
        // ownership is transferred to the returned PinnedBuffer.
        std::mem::forget(buffer);
        Ok(BufferImpl::new(slice, range, self.clone()))
    }

    pub fn allocate_buffer_sync_owned(self: &Arc<Self>, size: usize) -> OwnedBuffer {
        loop {
            match self.try_allocate_buffer_owned(size) {
                Ok(buffer) => return buffer,
                Err(listener) => listener.wait(),
            }
        }
    }
}

impl Drop for PinnedBufferAllocator {
    fn drop(&mut self) {
        let mut inner = self.inner.lock();
        for chunk in &mut inner.chunks {
            if let Some(pmt) = chunk.pmt.take() {
                // SAFETY: Allocator is being dropped, no DMA in flight.
                let _ = unsafe { pmt.unpin() };
            }
        }
    }
}

impl BufferAllocatorTrait for PinnedBufferAllocator {
    fn free_buffer(&self, range: Range<usize>) {
        self.free_buffer(range);
    }

    fn identifier(&self) -> usize {
        std::ptr::from_ref(self).addr()
    }

    fn is_trusted(&self) -> bool {
        false
    }

    fn vmo(&self) -> Option<Arc<zx::Vmo>> {
        self.vmo()
    }

    fn paddrs(&self, range: &Range<usize>) -> Option<(&[zx_paddr_t], u64)> {
        self.paddrs(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake_bti::FakeBti;
    use zx::Rights;

    #[fuchsia::test]
    async fn test_pinned_buffer() {
        let fake_bti = FakeBti::create().expect("failed to create fake BTI");
        fake_bti.set_paddrs(&[4096, 8192]);
        let source = BufferSource::new(8192);
        // Chunk size of 4096 so 8192 bytes = 2 chunks. Always keep chunk 0 pinned.
        let allocator = Arc::new(PinnedBufferAllocator::with_chunk_size(
            512,
            source,
            fake_bti.duplicate_handle(Rights::SAME_RIGHTS).unwrap(),
            4096,
            4096,
        ));

        assert!(!allocator.is_trusted());
        assert!(allocator.vmo().is_some());
        assert!(BufferAllocatorTrait::vmo(allocator.as_ref()).is_some());

        // First allocation in chunk 0 (which is pre-pinned at initialization).
        let buf = allocator.allocate_buffer(512).await;
        assert_eq!(buf.range(), 0..512);
        assert_eq!(buf.contiguity(), Some(4096));
        assert_eq!(buf.paddrs(), Some(&[4096][..]));
        assert!(buf.try_as_slice().is_none());
        assert!(buf.vmo().is_some());

        // Second allocation in the same chunk shares the existing pin.
        let buf2 = allocator.allocate_buffer_sync(512);
        assert_eq!(buf2.range(), 512..1024);
        assert_eq!(buf2.contiguity(), Some(4096));
        assert_eq!(buf2.paddrs(), Some(&[4096][..]));

        // Allocation in chunk 1 pins chunk 1 dynamically.
        let buf_chunk1 = allocator.allocate_buffer_sync_owned(4096);
        assert_eq!(buf_chunk1.range(), 4096..8192);
        assert_eq!(buf_chunk1.contiguity(), Some(4096));
        assert_eq!(buf_chunk1.paddrs(), Some(&[8192][..]));

        // Dropping buf_chunk1 unpins chunk 1 because chunk 1 > 0.
        std::mem::drop(buf_chunk1);

        // Dropping chunk 0 buffers leaves chunk 0 pinned because chunk 0 is always pinned.
        std::mem::drop(buf);
        std::mem::drop(buf2);

        // Next allocation reuses chunk 0 and is immediately pinned without touching chunk 1.
        let buf3 = allocator.allocate_buffer(512).await;
        assert_eq!(buf3.range(), 0..512);
        assert_eq!(buf3.contiguity(), Some(4096));
        assert_eq!(buf3.paddrs(), Some(&[4096][..]));
    }

    #[fuchsia::test]
    async fn test_spanning_chunks() {
        let fake_bti = FakeBti::create().expect("failed to create fake BTI");
        fake_bti.set_paddrs(&[4096, 8192]);
        let source = BufferSource::new(8192);
        // Chunk size 4096, contiguity 4096 -> 2 chunks.
        let allocator = PinnedBufferAllocator::with_chunk_size(
            512,
            source,
            fake_bti.duplicate_handle(Rights::SAME_RIGHTS).unwrap(),
            4096,
            4096,
        );

        // An 8192-byte allocation spans across chunk 0 and chunk 1.
        let mut buf = allocator.allocate_buffer(8192).await;
        assert_eq!(buf.range(), 0..8192);
        assert_eq!(buf.contiguity(), Some(4096));
        assert_eq!(buf.paddrs(), Some(&[4096, 8192][..]));

        // Write and read data across both chunks.
        buf.as_mut_ptr_slice().fill(0xab);
        assert_eq!(buf.as_ptr_slice().to_vec(), vec![0xab; 8192]);

        // Dropping the spanning buffer unpins chunk 1 while chunk 0 remains pinned.
        std::mem::drop(buf);

        // Next allocation in chunk 0 is immediately available with chunk 0's paddr.
        let buf_c0 = allocator.allocate_buffer_sync(512);
        assert_eq!(buf_c0.range(), 0..512);
        assert_eq!(buf_c0.paddrs(), Some(&[4096][..]));
    }

    #[fuchsia::test]
    async fn test_multiple_chunks_independent_lifecycle() {
        let fake_bti = FakeBti::create().expect("failed to create fake BTI");
        // Chunk 0 gets 0x1000 at init.
        // Chunk 1 gets 0x2000 on first pin.
        // Chunk 2 gets 0x3000 on pin.
        // Chunk 1 gets 0x4000 on repin.
        fake_bti.set_paddrs(&[0x1000, 0x2000, 0x3000, 0x4000]);
        let source = BufferSource::new(16384);
        // 4 chunks of 4096 bytes.
        let allocator = PinnedBufferAllocator::with_chunk_size(
            512,
            source,
            fake_bti.duplicate_handle(Rights::SAME_RIGHTS).unwrap(),
            4096,
            4096,
        );

        // Fill chunk 0.
        let buf0 = allocator.allocate_buffer_sync(4096);
        assert_eq!(buf0.paddrs(), Some(&[0x1000][..]));

        // Allocate in chunk 1 and chunk 2.
        let buf1 = allocator.allocate_buffer_sync(4096);
        assert_eq!(buf1.paddrs(), Some(&[0x2000][..]));
        let buf2 = allocator.allocate_buffer_sync(4096);
        assert_eq!(buf2.paddrs(), Some(&[0x3000][..]));

        // Drop buf1: chunk 1 is unpinned, but chunk 2 remains pinned.
        std::mem::drop(buf1);
        assert_eq!(buf2.paddrs(), Some(&[0x3000][..]));

        // Allocate again: lowest-offset-first reuses chunk 1, dynamically re-pinning it.
        let buf1_again = allocator.allocate_buffer_sync(4096);
        assert_eq!(buf1_again.range(), 4096..8192);
        assert_eq!(buf1_again.paddrs(), Some(&[0x4000][..]));

        // Dropping buf2 unpins chunk 2.
        std::mem::drop(buf2);

        // Dropping buf1_again unpins chunk 1.
        std::mem::drop(buf1_again);

        // Dropping buf0 leaves chunk 0 pinned.
        std::mem::drop(buf0);

        // Chunk 0 is still pinned and retains 0x1000.
        let buf0_again = allocator.allocate_buffer_sync(512);
        assert_eq!(buf0_again.paddrs(), Some(&[0x1000][..]));
    }

    #[fuchsia::test]
    async fn test_clean_transfer_buffer() {
        let fake_bti = FakeBti::create().expect("failed to create fake BTI");
        fake_bti.set_paddrs(&[4096, 8192]);
        let source = BufferSource::new(8192);
        let allocator = PinnedBufferAllocator::with_chunk_size(
            512,
            source,
            fake_bti.duplicate_handle(Rights::SAME_RIGHTS).unwrap(),
            4096,
            4096,
        );

        let buf = allocator.allocate_buffer(4096).await;
        // Clean unallocated memory while a buffer is active.
        allocator.clean_transfer_buffer();
        // Buffer is still valid and usable.
        assert_eq!(buf.paddrs(), Some(&[4096][..]));
    }

    #[fuchsia::test]
    async fn test_concurrent_pinned_allocations() {
        use fuchsia_async as fasync;
        let fake_bti = FakeBti::create().expect("failed to create fake BTI");
        let source = BufferSource::new(16384);
        let allocator = Arc::new(PinnedBufferAllocator::with_chunk_size(
            512,
            source,
            fake_bti.duplicate_handle(Rights::SAME_RIGHTS).unwrap(),
            4096,
            4096,
        ));

        let mut tasks = Vec::new();
        for i in 0..8 {
            let alloc = allocator.clone();
            tasks.push(async move {
                let mut buf = alloc.allocate_buffer(512).await;
                assert!(buf.paddrs().is_some());
                buf.as_mut_ptr_slice().fill(i as u8);
                fasync::Timer::new(std::time::Duration::from_millis(5)).await;
                assert_eq!(buf.as_ptr_slice().to_vec(), vec![i as u8; 512]);
            });
        }
        futures::future::join_all(tasks).await;
    }

    #[fuchsia::test]
    async fn test_buffer_subslicing() {
        let fake_bti = FakeBti::create().expect("failed to create fake BTI");
        fake_bti.set_paddrs(&[4096]);
        let source = BufferSource::new(4096);
        let allocator = PinnedBufferAllocator::new(
            512,
            source,
            fake_bti.duplicate_handle(Rights::SAME_RIGHTS).unwrap(),
            4096,
        );

        let mut buf = allocator.allocate_buffer(1024).await;
        buf.as_mut_ptr_slice().fill(0x77);

        let sub_ref = buf.subslice(100..200);
        assert_eq!(sub_ref.len(), 100);
        assert_eq!(sub_ref.to_vec(), vec![0x77; 100]);

        let buf_ref = buf.as_ref();
        let (left, right) = buf_ref.split_at(512);
        assert_eq!(left.len(), 512);
        assert_eq!(right.len(), 512);
    }
}
