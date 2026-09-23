// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{BLOCK_SIZE, Extents, File};
use anyhow::{Error, anyhow, bail};
use fuchsia_sync::Mutex;
use std::cmp::{Ordering, Reverse, max, min};
use std::collections::BinaryHeap;
use std::ops::{ControlFlow, Range};
use std::sync::Arc;

/// Maximum number of bytes to read per single storage buffer request (1 MiB).
pub const MAX_READ_BUFFER_SIZE: usize = 1024 * 1024;

pub use storage_device::SplittableBuffer;
pub use storage_device::buffer::OwnedBuffer;

/// Interface for block storage services providing read buffers and block read operations.
pub trait BlockService: Send + Sync {
    /// Allocates a read buffer of at most `max_len` bytes (`OwnedBuffer`).
    ///
    /// `max_len` must be greater than zero. Implementations must guarantee that the returned
    /// buffer satisfies `0 < buffer.len() <= max_len` and that `buffer.len()` is a multiple of
    /// `BLOCK_SIZE`.
    fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer;

    /// Submits a read for the specified range from `device_offset` into `dest_buffer`.
    /// When the read completes, `on_complete` is invoked with the result containing the buffer.
    fn read_blocks(
        &self,
        device_offset: u64,
        dest_buffer: OwnedBuffer,
        on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
    ) -> Result<(), Error>;
}

struct BufferedChunk {
    index: usize,
    buffer: OwnedBuffer,
}

impl PartialEq for BufferedChunk {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl Eq for BufferedChunk {}

impl PartialOrd for BufferedChunk {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BufferedChunk {
    fn cmp(&self, other: &Self) -> Ordering {
        self.index.cmp(&other.index)
    }
}

/// Tracks the state of a streaming read operation.
///
/// Ensures out-of-order completions from parallel block read requests across extents and buffer
/// allocations are delivered to `callback` in strict ascending logical order.
struct ReadContext<F>
where
    F: FnMut(Result<OwnedBuffer, Error>) -> ControlFlow<()> + Send + 'static,
{
    callback: Option<F>,

    /// Sequence index (`k`) of the next chunk due to be delivered to `callback`.
    next_expected_chunk: usize,

    /// Out-of-order successful completions waiting for earlier chunks to finish before delivery.
    buffered_chunks: BinaryHeap<Reverse<BufferedChunk>>,

    /// Total number of buffers allocated across all iterations of `read_aligned_range`.
    total_chunks: Option<usize>,
}

impl<F> ReadContext<F>
where
    F: FnMut(Result<OwnedBuffer, Error>) -> ControlFlow<()> + Send + 'static,
{
    /// Invoked when the asynchronous read for chunk sequence index `chunk_index` completes.
    ///
    /// Buffers out-of-order completions and delivers consecutive chunks (`0, 1, 2, ...`) in order
    /// to `callback`. Any error immediately aborts future deliveries and clears buffered chunks.
    fn on_block_read_completed(
        context: &Mutex<ReadContext<F>>,
        chunk_index: usize,
        res: Result<OwnedBuffer, Error>,
    ) {
        let mut guard = context.lock();
        if guard.callback.is_none() {
            return;
        }

        let mut current_buf = match res {
            Ok(buf) => buf,
            Err(e) => {
                guard.buffered_chunks.clear();
                let mut cb = guard.callback.take().unwrap();
                drop(guard);
                let _ = cb(Err(e));
                return;
            }
        };

        if chunk_index != guard.next_expected_chunk {
            guard
                .buffered_chunks
                .push(Reverse(BufferedChunk { index: chunk_index, buffer: current_buf }));
            return;
        }

        loop {
            guard.next_expected_chunk += 1;
            let cb = guard.callback.as_mut().unwrap();
            if cb(Ok(current_buf)).is_break() {
                guard.buffered_chunks.clear();
                guard.callback = None;
                return;
            }

            if let Some(top) = guard.buffered_chunks.peek()
                && top.0.index == guard.next_expected_chunk
            {
                current_buf = guard.buffered_chunks.pop().unwrap().0.buffer;
            } else {
                break;
            }
        }
    }
}

impl<F> Drop for ReadContext<F>
where
    F: FnMut(Result<OwnedBuffer, Error>) -> ControlFlow<()> + Send + 'static,
{
    fn drop(&mut self) {
        if let Some(mut callback) = self.callback.take() {
            let incomplete = match self.total_chunks {
                Some(total) => {
                    self.next_expected_chunk != total || !self.buffered_chunks.is_empty()
                }
                None => true,
            };
            if incomplete {
                let _ = callback(Err(anyhow!("ReadContext dropped before completion")));
            }
        }
    }
}

/// Reads the specified block-aligned logical `range` (`start..end`) into one or more `OwnedBuffer`
/// chunks with zero copies, streaming each chunk sequentially to `callback`.
///
/// Both `range.start` and `range.end` must be multiples of `BLOCK_SIZE`. Callers requiring
/// unaligned sub-ranges must align `range` to block boundaries and handle leading or trailing
/// byte trimming/zeroing in `callback`.
///
/// Allocates buffers and submits block read requests immediately inside a loop until `range`
/// is fully covered. If `service.allocate_buffer` returns a partial buffer or blocks due to memory
/// pool exhaustion, the function makes progress by allocating and submitting what is available,
/// then blocking inside `allocate_buffer` when needed until background completions free memory back
/// to the pool. Out-of-order completions across requests are buffered so `callback` always receives
/// chunks strictly in ascending logical offset sequence.
pub fn read_aligned_range<F>(
    extents: &Extents,
    range: Range<u64>,
    service: &(impl BlockService + ?Sized),
    mut callback: F,
) where
    F: FnMut(Result<OwnedBuffer, Error>) -> ControlFlow<()> + Send + 'static,
{
    let mut offset = range.start;
    let end = range.end;
    assert_eq!(
        offset % BLOCK_SIZE,
        0,
        "read_aligned_range: range.start ({offset}) must be block aligned ({BLOCK_SIZE})"
    );
    assert_eq!(
        end % BLOCK_SIZE,
        0,
        "read_aligned_range: range.end ({end}) must be block aligned ({BLOCK_SIZE})"
    );

    if range.is_empty() {
        let _ = callback(Err(anyhow!(
            "read_aligned_range: range {range:?} must have a non-zero length"
        )));
        return;
    }

    let context = Arc::new(Mutex::new(ReadContext {
        callback: Some(callback),
        next_expected_chunk: 0,
        buffered_chunks: BinaryHeap::new(),
        total_chunks: None,
    }));

    let mut next_chunk_index = 0usize;

    while offset < end {
        if context.lock().callback.is_none() {
            break;
        }

        let total_len = min(end - offset, MAX_READ_BUFFER_SIZE as u64) as usize;
        let buffer = service.allocate_buffer(total_len);
        let actual_len = buffer.len();
        assert!(actual_len > 0 && actual_len <= total_len && actual_len % BLOCK_SIZE as usize == 0);

        let actual_end = offset + actual_len as u64;

        let chunk_index = next_chunk_index;
        next_chunk_index += 1;

        let context_clone = context.clone();
        if let Err(e) = buffer.split(
            |splittable| {
                for extent in extents.iter_extents(offset) {
                    if extent.logical_range().start >= actual_end {
                        break;
                    }
                    let slice_start = max(extent.logical_range().start, offset);
                    let slice_end = min(extent.logical_range().end, actual_end);
                    let len_in_buf = (slice_end - slice_start) as usize;

                    if let Some(dev_offset) = extent.device_offset() {
                        let extent_dev_offset =
                            dev_offset + (slice_start - extent.logical_range().start);
                        let (child_buf, sub_handle) = splittable.take_prefix(len_in_buf);
                        service.read_blocks(
                            extent_dev_offset,
                            child_buf,
                            Box::new(move |res| {
                                sub_handle.merge(|| {
                                    let _buf = res?;
                                    Ok(())
                                });
                            }),
                        )?;
                    } else {
                        splittable.fill_zeros(len_in_buf);
                    }
                }

                let remaining_len = splittable.remaining_range().len();
                if remaining_len > 0 {
                    bail!(
                        "read_aligned_range: requested range extends {remaining_len} bytes \
                         beyond the end of extents mappings"
                    );
                }

                Ok(())
            },
            move |res| ReadContext::on_block_read_completed(&context_clone, chunk_index, res),
        ) {
            ReadContext::on_block_read_completed(&context, chunk_index, Err(e));
            return;
        }

        offset = actual_end;
    }

    context.lock().total_chunks = Some(next_chunk_index);
}

/// Reads data for `dest_buffer` starting at `logical_offset` according to `extents` from `service`.
///
/// If the range is contiguous within a single extent, issues a single read. If the range spans
/// multiple extents or holes, splits `dest_buffer` across the extents and reconstructs the
/// original buffer when all reads complete.
pub fn read_buffer_from_extents(
    extents: &Extents,
    logical_offset: u64,
    dest_buffer: OwnedBuffer,
    service: &(impl BlockService + ?Sized),
    on_complete: impl FnOnce(Result<OwnedBuffer, Error>) + Send + 'static,
) -> Result<(), Error> {
    let total_len = dest_buffer.len() as u64;
    if total_len == 0 {
        on_complete(Ok(dest_buffer));
        return Ok(());
    }

    dest_buffer.split(
        |splittable| {
            let end_offset = logical_offset + total_len;

            // Fast path: Check if the entire range is covered by a single contiguous extent.
            if let Some(extent) = extents.iter_extents(logical_offset).next() {
                let logical = extent.logical_range();
                if logical.start <= logical_offset && logical.end >= end_offset {
                    if let Some(dev_offset) = extent.device_offset() {
                        let extent_dev_offset = dev_offset + (logical_offset - logical.start);
                        let (child_buf, sub_handle) = splittable.take_prefix(total_len as usize);
                        return service.read_blocks(
                            extent_dev_offset,
                            child_buf,
                            Box::new(move |res| {
                                sub_handle.merge(|| {
                                    let _buf = res?;
                                    Ok(())
                                });
                            }),
                        );
                    } else {
                        // Sparse hole covering the full range.
                        splittable.fill_zeros(total_len as usize);
                        return Ok(());
                    }
                }
            }

            // Multi-extent path using SplittableBuffer.
            let mut current_offset = logical_offset;
            for extent in extents.iter_extents(logical_offset) {
                if extent.logical_range().start >= end_offset {
                    break;
                }
                let slice_start = max(extent.logical_range().start, current_offset);
                let slice_end = min(extent.logical_range().end, end_offset);
                if slice_start >= slice_end {
                    continue;
                }

                if slice_start > current_offset {
                    let hole_len = (slice_start - current_offset) as usize;
                    splittable.fill_zeros(hole_len);
                }

                let len_in_buf = (slice_end - slice_start) as usize;
                if let Some(dev_offset) = extent.device_offset() {
                    let extent_dev_offset =
                        dev_offset + (slice_start - extent.logical_range().start);
                    let (child_buf, sub_handle) = splittable.take_prefix(len_in_buf);
                    service.read_blocks(
                        extent_dev_offset,
                        child_buf,
                        Box::new(move |res| {
                            sub_handle.merge(|| {
                                let _buf = res?;
                                Ok(())
                            });
                        }),
                    )?;
                } else {
                    splittable.fill_zeros(len_in_buf);
                }
                current_offset = slice_end;
            }

            if current_offset < end_offset {
                let hole_len = (end_offset - current_offset) as usize;
                splittable.fill_zeros(hole_len);
            }

            Ok(())
        },
        on_complete,
    )
}

/// A [`BlockService`] that maps logical block offsets through the extents of a mapped [`File`]
/// in a parent session.
pub struct ChildBlockService<S: ?Sized> {
    parent_service: Arc<S>,
    file: Arc<File>,
}

impl<S: BlockService + ?Sized> ChildBlockService<S> {
    pub fn new(parent_service: Arc<S>, file: Arc<File>) -> Self {
        Self { parent_service, file }
    }

    pub fn file(&self) -> &Arc<File> {
        &self.file
    }
}

impl<S: BlockService + ?Sized> BlockService for ChildBlockService<S> {
    fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
        self.parent_service.allocate_buffer(max_len)
    }

    fn read_blocks(
        &self,
        device_offset: u64,
        dest_buffer: OwnedBuffer,
        on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
    ) -> Result<(), Error> {
        read_buffer_from_extents(
            self.file.extents(),
            device_offset,
            dest_buffer,
            self.parent_service.as_ref(),
            on_complete,
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Extent, Transform};
    use std::cmp::min;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;
    use storage_device::buffer_allocator::{BufferAllocator, BufferSource};

    pub(crate) struct FakeBlockService {
        allocator: Arc<BufferAllocator>,
        device_data: Mutex<Vec<u8>>,
        cap: Option<usize>,
    }

    impl FakeBlockService {
        pub(crate) fn new(device_data: Vec<u8>) -> Self {
            Self::new_with_cap(device_data, None)
        }

        pub(crate) fn new_with_cap(device_data: Vec<u8>, cap: Option<usize>) -> Self {
            Self::new_with_pool_size(device_data, 1024 * 1024, cap)
        }

        pub(crate) fn new_with_pool_size(
            device_data: Vec<u8>,
            pool_size: usize,
            cap: Option<usize>,
        ) -> Self {
            let source = BufferSource::new(pool_size);
            let allocator = Arc::new(BufferAllocator::new(4096, source));
            Self { allocator, device_data: Mutex::new(device_data), cap }
        }
    }

    impl BlockService for FakeBlockService {
        fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
            assert!(max_len > 0, "FakeBlockService::allocate_buffer: max_len must be > 0");
            let len = match self.cap {
                Some(cap) => min(max_len, cap),
                None => max_len,
            };
            self.allocator.allocate_buffer_sync_owned(len)
        }

        fn read_blocks(
            &self,
            device_offset: u64,
            mut dest_buffer: OwnedBuffer,
            on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
        ) -> Result<(), Error> {
            assert_eq!(
                device_offset % 512,
                0,
                "FakeBlockService::read_blocks: device_offset ({device_offset}) must be 512-byte \
                 aligned"
            );
            let len = dest_buffer.len();
            assert_eq!(
                len % BLOCK_SIZE as usize,
                0,
                "FakeBlockService::read_blocks: dest_buffer.len() ({len}) must be block aligned"
            );
            let start = device_offset as usize;
            let end = start + dest_buffer.len();
            let data = self.device_data.lock();
            dest_buffer.copy_from_slice(&data[start..end]);
            on_complete(Ok(dest_buffer));
            Ok(())
        }
    }

    #[test]
    #[should_panic(expected = "must be block aligned")]
    fn test_read_aligned_range_unaligned_panics() {
        let service = Arc::new(FakeBlockService::new(vec![0u8; 8192]));
        let mappings = Extents::try_new([Extent::new(0..8192, Some(0))], 0).unwrap();

        read_aligned_range(&mappings, 1000..5000, &*service, |_| ControlFlow::Continue(()));
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges)]
    fn test_read_aligned_range_empty_range_errors() {
        let service = FakeBlockService::new(vec![0u8; 8192]);
        let mappings = Extents::try_new([Extent::new(0..8192, Some(0))], 0).unwrap();

        let err_count = Arc::new(AtomicUsize::new(0));
        let err_count_clone = err_count.clone();
        read_aligned_range(&mappings, 4096..4096, &service, move |res| {
            assert!(res.is_err());
            err_count_clone.fetch_add(1, Ordering::Relaxed);
            ControlFlow::Continue(())
        });
        assert_eq!(err_count.load(Ordering::Relaxed), 1);

        let err_count_clone2 = err_count.clone();
        read_aligned_range(&mappings, 8192..4096, &service, move |res| {
            assert!(res.is_err());
            err_count_clone2.fetch_add(1, Ordering::Relaxed);
            ControlFlow::Continue(())
        });
        assert_eq!(err_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_read_aligned_range_beyond_extents_errors() {
        let service = FakeBlockService::new(vec![0u8; 16384]);
        // extents only cover 0..8192, but we request 0..16384.
        let mappings = Extents::try_new([Extent::new(0..8192, Some(0))], 0).unwrap();

        let err_occurred = Arc::new(AtomicBool::new(false));
        let err_occurred_clone = err_occurred.clone();
        read_aligned_range(&mappings, 0..16384, &service, move |res| {
            if res.is_err() {
                err_occurred_clone.store(true, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });
        assert!(err_occurred.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_aligned_range_single_extent() {
        let mut device_data = vec![0u8; 16384];
        device_data[4096..8192].copy_from_slice(&[42u8; 4096]);
        let service = Arc::new(FakeBlockService::new(device_data));

        let mappings = Extents::try_new([Extent::new(0..4096, Some(4096))], 0).unwrap();

        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        read_aligned_range(&mappings, 0..4096, &*service, move |res| {
            let buffer = res.expect("read_aligned_range should succeed");
            assert_eq!(buffer.len(), 4096);
            assert!(buffer.as_ptr_slice().iter_as::<u8>().all(|b| b == 42));
            completed_clone.store(true, Ordering::Relaxed);
            ControlFlow::Continue(())
        });

        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_aligned_range_multi_extent() {
        let mut device_data = vec![0u8; 16384];
        device_data[0..4096].fill(1);
        device_data[8192..12288].fill(2);
        let service = Arc::new(FakeBlockService::new(device_data));

        let mappings = Extents::try_new(
            [Extent::new(0..4096, Some(0)), Extent::new(4096..8192, Some(8192))],
            0,
        )
        .unwrap();

        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();
        let mut received_bytes = Vec::new();

        read_aligned_range(&mappings, 0..8192, &*service, move |res| {
            let buffer = res.expect("read_aligned_range should succeed");
            buffer.as_ptr_slice().append_to(&mut received_bytes);
            if received_bytes.len() == 8192 {
                assert!(received_bytes[0..4096].iter().all(|&b| b == 1));
                assert!(received_bytes[4096..8192].iter().all(|&b| b == 2));
                completed_clone.store(true, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });

        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_aligned_range_sparse_extent() {
        let service = Arc::new(FakeBlockService::new(vec![0u8; 4096]));

        let mappings = Extents::try_new([Extent::new(0..4096, None)], 0).unwrap();

        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        read_aligned_range(&mappings, 0..4096, &*service, move |res| {
            let buffer = res.expect("read_aligned_range should succeed");
            assert_eq!(buffer.len(), 4096);
            assert!(buffer.as_ptr_slice().iter_as::<u64>().all(|b| b == 0));
            completed_clone.store(true, Ordering::Relaxed);
            ControlFlow::Continue(())
        });

        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_aligned_range_partial_buffer_chaining() {
        let mut device_data = vec![0u8; 16384];
        device_data[0..4096].fill(1);
        device_data[4096..8192].fill(2);
        // Request up to 8192 bytes, but cap allocator to 4096 bytes per round.
        let service = Arc::new(FakeBlockService::new_with_cap(device_data, Some(4096)));

        let mappings = Extents::try_new([Extent::new(0..8192, Some(0))], 0).unwrap();

        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();
        let mut chunk_count = 0;
        let mut received_bytes = Vec::new();

        read_aligned_range(&mappings, 0..8192, &*service, move |res| {
            let buffer = res.expect("read_aligned_range should succeed");
            chunk_count += 1;
            buffer.as_ptr_slice().append_to(&mut received_bytes);
            if received_bytes.len() == 8192 {
                assert_eq!(chunk_count, 2, "Should have streamed in 2 rounds of 4096 bytes");
                assert!(received_bytes[0..4096].iter().all(|&b| b == 1));
                assert!(received_bytes[4096..8192].iter().all(|&b| b == 2));
                completed_clone.store(true, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });

        assert!(completed.load(Ordering::Relaxed));
    }

    struct OutOfOrderBlockService {
        inner: FakeBlockService,
        delayed: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    }

    impl BlockService for OutOfOrderBlockService {
        fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
            self.inner.allocate_buffer(max_len)
        }

        fn read_blocks(
            &self,
            device_offset: u64,
            dest_buffer: OwnedBuffer,
            on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
        ) -> Result<(), Error> {
            if device_offset == 0 {
                // Delay chunk 0 until after both chunk 1 and chunk 2 complete.
                let inner = &self.inner;
                let data = inner.device_data.lock();
                let start = device_offset as usize;
                let end = start + dest_buffer.len();
                let mut buf = dest_buffer;
                buf.copy_from_slice(&data[start..end]);
                self.delayed.lock().push(Box::new(move || on_complete(Ok(buf))));
                Ok(())
            } else if device_offset == 4096 {
                // Immediately complete chunk 1, adding index 1 to `buffered_chunks`.
                self.inner.read_blocks(device_offset, dest_buffer, on_complete)?;
                Ok(())
            } else {
                // Immediately complete chunk 2, adding index 2 to `buffered_chunks` and exercising
                // `BufferedChunk::cmp` when sorting multiple out-of-order elements in the heap.
                // Then execute delayed chunk 0 to trigger sequential delivery of 0, 1, and 2.
                self.inner.read_blocks(device_offset, dest_buffer, on_complete)?;
                let mut delayed = self.delayed.lock();
                for cb in delayed.drain(..) {
                    cb();
                }
                Ok(())
            }
        }
    }

    #[test]
    fn test_read_aligned_range_out_of_order_reordering() {
        let mut device_data = vec![0u8; 16384];
        device_data[0..4096].fill(10);
        device_data[4096..8192].fill(20);
        device_data[8192..12288].fill(30);
        let inner = FakeBlockService::new_with_cap(device_data, Some(4096));
        let service = Arc::new(OutOfOrderBlockService { inner, delayed: Mutex::new(Vec::new()) });

        let mappings = Extents::try_new(
            [
                Extent::new(0..4096, Some(0)),
                Extent::new(4096..8192, Some(4096)),
                Extent::new(8192..12288, Some(8192)),
            ],
            0,
        )
        .unwrap();

        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();
        let mut received_chunks = Vec::new();

        read_aligned_range(&mappings, 0..12288, &*service, move |res| {
            let buffer = res.expect("read_aligned_range should succeed");
            let first_byte = buffer.as_ptr_slice().to_vec()[0];
            received_chunks.push(first_byte);
            if received_chunks.len() == 3 {
                // Verify strict delivery order: chunk 0 (10), then chunk 1 (20), then chunk 2 (30).
                assert_eq!(received_chunks, vec![10, 20, 30]);
                completed_clone.store(true, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });

        assert!(completed.load(Ordering::Relaxed));
    }

    struct ThreadedBlockService {
        inner: FakeBlockService,
    }

    impl BlockService for ThreadedBlockService {
        fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
            self.inner.allocate_buffer(max_len)
        }

        fn read_blocks(
            &self,
            device_offset: u64,
            mut dest_buffer: OwnedBuffer,
            on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
        ) -> Result<(), Error> {
            let start = device_offset as usize;
            let end = start + dest_buffer.len();
            let data = self.inner.device_data.lock();
            dest_buffer.copy_from_slice(&data[start..end]);
            drop(data);

            // Spawn on a background thread so the upfront read_range loop can run right ahead,
            // and block on allocate_buffer when memory is full until this thread drops its buffer.
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(15));
                on_complete(Ok(dest_buffer));
            });
            Ok(())
        }
    }

    #[test]
    fn test_read_aligned_range_limited_memory_blocking_allocator() {
        let mut device_data = vec![0u8; 16384];
        device_data[0..4096].fill(1);
        device_data[4096..8192].fill(2);
        device_data[8192..12288].fill(3);
        device_data[12288..16384].fill(4);

        // Pool size is only 8192 bytes, capped at 4096 bytes per buffer.
        // Requesting 16384 bytes will allocate buffers 0 and 1 (using all 8192 bytes of the pool),
        // submit their reads right away inside read_aligned_range's while loop, and then
        // block when trying to allocate buffer 2 until background threads complete and free
        // buffer 0.
        let inner = FakeBlockService::new_with_pool_size(device_data, 8192, Some(4096));
        let service = Arc::new(ThreadedBlockService { inner });

        let mappings = Extents::try_new([Extent::new(0..16384, Some(0))], 0).unwrap();

        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();
        let mut received_bytes = Vec::new();

        read_aligned_range(&mappings, 0..16384, &*service, move |res| {
            let buffer = res.expect("read_aligned_range should succeed");
            buffer.as_ptr_slice().append_to(&mut received_bytes);
            if received_bytes.len() == 16384 {
                assert!(received_bytes[0..4096].iter().all(|&b| b == 1));
                assert!(received_bytes[4096..8192].iter().all(|&b| b == 2));
                assert!(received_bytes[8192..12288].iter().all(|&b| b == 3));
                assert!(received_bytes[12288..16384].iter().all(|&b| b == 4));
                completed_clone.store(true, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });

        // Wait briefly for background threads to finish completing all 4 chunks.
        for _ in 0..100 {
            if completed.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_aligned_range_max_read_buffer_size_capping() {
        struct CapturingBlockService {
            inner: FakeBlockService,
            requested_lens: Mutex<Vec<usize>>,
        }

        impl BlockService for CapturingBlockService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                self.requested_lens.lock().push(max_len);
                self.inner.allocate_buffer(max_len)
            }

            fn read_blocks(
                &self,
                device_offset: u64,
                dest_buffer: OwnedBuffer,
                on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                self.inner.read_blocks(device_offset, dest_buffer, on_complete)
            }
        }

        let inner = FakeBlockService::new(vec![0u8; MAX_READ_BUFFER_SIZE + 8192]);
        let service = CapturingBlockService { inner, requested_lens: Mutex::new(Vec::new()) };
        let mappings =
            Extents::try_new([Extent::new(0..(MAX_READ_BUFFER_SIZE + 8192) as u64, Some(0))], 0)
                .unwrap();

        read_aligned_range(&mappings, 0..(MAX_READ_BUFFER_SIZE + 8192) as u64, &service, |_| {
            ControlFlow::Continue(())
        });

        assert_eq!(*service.requested_lens.lock(), vec![MAX_READ_BUFFER_SIZE, 8192]);
    }

    #[test]
    fn test_read_aligned_range_sync_read_blocks_error() {
        struct SyncErrorBlockService(FakeBlockService);
        impl BlockService for SyncErrorBlockService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                self.0.allocate_buffer(max_len)
            }
            fn read_blocks(
                &self,
                _offset: u64,
                _dest: OwnedBuffer,
                _on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                Err(anyhow!("synchronous read_blocks failure"))
            }
        }
        let service = SyncErrorBlockService(FakeBlockService::new(vec![0u8; 4096]));
        let mappings = Extents::try_new([Extent::new(0..4096, Some(0))], 0).unwrap();

        let err_received = Arc::new(AtomicBool::new(false));
        let err_clone = err_received.clone();
        read_aligned_range(&mappings, 0..4096, &service, move |res| {
            assert!(res.is_err());
            err_clone.store(true, Ordering::Relaxed);
            ControlFlow::Continue(())
        });
        assert!(err_received.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_aligned_range_async_callback_errors() {
        struct AsyncErrorBlockService(FakeBlockService, u64);
        impl BlockService for AsyncErrorBlockService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                self.0.allocate_buffer(max_len)
            }
            fn read_blocks(
                &self,
                device_offset: u64,
                dest_buffer: OwnedBuffer,
                on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                if device_offset == self.1 {
                    on_complete(Err(anyhow!("async block read error on offset {device_offset}")));
                    Ok(())
                } else {
                    self.0.read_blocks(device_offset, dest_buffer, on_complete)
                }
            }
        }

        // Test in-order error on chunk 0.
        let service = AsyncErrorBlockService(FakeBlockService::new(vec![0u8; 8192]), 0);
        let mappings = Extents::try_new([Extent::new(0..8192, Some(0))], 0).unwrap();
        let err_count = Arc::new(AtomicUsize::new(0));
        let err_clone = err_count.clone();
        read_aligned_range(&mappings, 0..8192, &service, move |res| {
            if res.is_err() {
                err_clone.fetch_add(1, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });
        assert_eq!(err_count.load(Ordering::Relaxed), 1);

        // Test out-of-order error where chunk 1 (4096) succeeds first, then chunk 0 (0) fails.
        let service = Arc::new(OutOfOrderBlockService {
            inner: FakeBlockService::new(vec![0u8; 8192]),
            delayed: Mutex::new(Vec::new()),
        });
        // We wrap OutOfOrderBlockService to make chunk 0 fail when drained.
        struct FailingDelayedService(Arc<OutOfOrderBlockService>);
        impl BlockService for FailingDelayedService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                self.0.allocate_buffer(max_len)
            }
            fn read_blocks(
                &self,
                device_offset: u64,
                dest_buffer: OwnedBuffer,
                on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                if device_offset == 0 {
                    self.0.delayed.lock().push(Box::new(move || {
                        on_complete(Err(anyhow!("delayed chunk 0 failure")))
                    }));
                    Ok(())
                } else {
                    let res = self.0.read_blocks(device_offset, dest_buffer, on_complete);
                    let mut delayed = self.0.delayed.lock();
                    for cb in delayed.drain(..) {
                        cb();
                    }
                    res
                }
            }
        }
        let failing_service = FailingDelayedService(service.clone());
        let err_count = Arc::new(AtomicUsize::new(0));
        let err_clone = err_count.clone();
        read_aligned_range(&mappings, 0..8192, &failing_service, move |res| {
            if res.is_err() {
                err_clone.fetch_add(1, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });
        let mut delayed = service.delayed.lock();
        for cb in delayed.drain(..) {
            cb();
        }
        assert_eq!(err_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_read_context_dropped_before_completion() {
        struct DroppingBlockService(Mutex<Vec<Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>>>);
        impl BlockService for DroppingBlockService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                FakeBlockService::new(vec![0u8; max_len]).allocate_buffer(max_len)
            }
            fn read_blocks(
                &self,
                _offset: u64,
                _dest: OwnedBuffer,
                on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                self.0.lock().push(on_complete);
                Ok(())
            }
        }
        let service = DroppingBlockService(Mutex::new(Vec::new()));
        let mappings = Extents::try_new([Extent::new(0..4096, Some(0))], 0).unwrap();

        let err_msg = Arc::new(Mutex::new(String::new()));
        let err_msg_clone = err_msg.clone();
        read_aligned_range(&mappings, 0..4096, &service, move |res| {
            if let Err(e) = res {
                *err_msg_clone.lock() = e.to_string();
            }
            ControlFlow::Continue(())
        });
        service.0.lock().clear();
        assert_eq!(*err_msg.lock(), "Read sub-request dropped before completion");
    }

    #[test]
    fn test_read_aligned_range_multi_iteration_error_break() {
        struct FirstIterSyncErrorService(FakeBlockService);
        impl BlockService for FirstIterSyncErrorService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                self.0.allocate_buffer(max_len)
            }
            fn read_blocks(
                &self,
                device_offset: u64,
                dest_buffer: OwnedBuffer,
                on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                if device_offset == 0 {
                    Err(anyhow!("sync failure on first iteration"))
                } else {
                    self.0.read_blocks(device_offset, dest_buffer, on_complete)
                }
            }
        }
        let inner = FakeBlockService::new_with_cap(vec![0u8; 8192], Some(4096));
        let service = FirstIterSyncErrorService(inner);
        let mappings = Extents::try_new([Extent::new(0..8192, Some(0))], 0).unwrap();

        let err_received = Arc::new(AtomicBool::new(false));
        let err_clone = err_received.clone();
        read_aligned_range(&mappings, 0..8192, &service, move |res| {
            if res.is_err() {
                err_clone.store(true, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });
        assert!(err_received.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_aligned_range_extent_beyond_actual_end_break() {
        let inner = FakeBlockService::new_with_cap(vec![0u8; 8192], Some(4096));
        let mappings = Extents::try_new(
            [Extent::new(0..4096, Some(0)), Extent::new(4096..8192, Some(4096))],
            0,
        )
        .unwrap();

        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        read_aligned_range(&mappings, 0..8192, &inner, move |res| {
            if res.is_ok() {
                count_clone.fetch_add(1, Ordering::Relaxed);
            }
            ControlFlow::Continue(())
        });
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_read_aligned_range_merged_extents() {
        let mut device_data = vec![0u8; 32768];
        device_data[4096..8192].fill(0xAA);
        device_data[8192..20480].fill(0xBB);
        let service = Arc::new(FakeBlockService::new(device_data));

        let mappings = Extents::try_new(
            [Extent::new(0..4096, Some(4096)), Extent::new(4096..16384, Some(8192))],
            0,
        )
        .unwrap();

        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        read_aligned_range(&mappings, 0..16384, &*service, move |res| {
            count_clone.fetch_add(1, Ordering::Relaxed);
            let buffer = res.expect("merged read should succeed");
            assert_eq!(buffer.len(), 16384);
            assert!(buffer.as_ptr_slice().subslice(0..4096).iter_as::<u8>().all(|b| b == 0xAA));
            assert!(buffer.as_ptr_slice().subslice(4096..16384).iter_as::<u8>().all(|b| b == 0xBB));
            completed_clone.store(true, Ordering::Relaxed);
            ControlFlow::Continue(())
        });

        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_aligned_range_callback_error_propagation() {
        let service = Arc::new(FakeBlockService::new(vec![0u8; 8192]));
        let mappings = Extents::try_new([Extent::new(0..8192, Some(0))], 0).unwrap();

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        read_aligned_range(&mappings, 0..8192, &*service, move |_res| {
            call_count_clone.fetch_add(1, Ordering::Relaxed);
            ControlFlow::Break(())
        });

        assert_eq!(call_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_read_buffer_from_extents_single_extent() {
        let mut device_data = vec![0u8; 16384];
        device_data[4096..8192].fill(0xAA);
        let service = Arc::new(FakeBlockService::new(device_data));

        // Logical 0..4096 maps to device 4096..8192.
        let mappings = Extents::try_new([Extent::new(0..4096, Some(4096))], 0).unwrap();

        let dest = service.allocate_buffer(4096);
        let (tx, rx) = std::sync::mpsc::channel();
        read_buffer_from_extents(&mappings, 0, dest, service.as_ref(), move |res| {
            tx.send(res).unwrap();
        })
        .unwrap();

        let buffer = rx.recv().unwrap().unwrap();
        assert_eq!(buffer.len(), 4096);
        assert!(buffer.as_ptr_slice().iter_as::<u8>().all(|b| b == 0xAA));
    }

    #[test]
    fn test_read_buffer_from_extents_sparse_hole_fast_path() {
        let service = Arc::new(FakeBlockService::new(vec![0u8; 16384]));

        // Logical 0..4096 is a sparse hole (None).
        let mappings = Extents::try_new([Extent::new(0..4096, None)], 0).unwrap();

        let dest = service.allocate_buffer(4096);
        let (tx, rx) = std::sync::mpsc::channel();
        read_buffer_from_extents(&mappings, 0, dest, service.as_ref(), move |res| {
            tx.send(res).unwrap();
        })
        .unwrap();

        let buffer = rx.recv().unwrap().unwrap();
        assert_eq!(buffer.len(), 4096);
        assert!(buffer.as_ptr_slice().iter_as::<u8>().all(|b| b == 0x00));
    }

    #[test]
    fn test_read_buffer_from_extents_multi_extent_and_holes() {
        let mut device_data = vec![0u8; 32768];
        device_data[0..4096].fill(0x11);
        device_data[8192..12288].fill(0x22);
        let service = Arc::new(FakeBlockService::new(device_data));

        // Child maps:
        // 0..4096 -> device 0..4096 (0x11)
        // 4096..8192 -> hole (0x00)
        // 8192..12288 -> device 8192..12288 (0x22)
        let mappings = Extents::try_new(
            [
                Extent::new(0..4096, Some(0)),
                Extent::new(4096..8192, None),
                Extent::new(8192..12288, Some(8192)),
            ],
            0,
        )
        .unwrap();

        let dest = service.allocate_buffer(12288);
        let (tx, rx) = std::sync::mpsc::channel();
        read_buffer_from_extents(&mappings, 0, dest, service.as_ref(), move |res| {
            tx.send(res).unwrap();
        })
        .unwrap();

        let buffer = rx.recv().unwrap().unwrap();
        assert_eq!(buffer.len(), 12288);
        assert!(buffer.as_ptr_slice().subslice(0..4096).iter_as::<u8>().all(|b| b == 0x11));
        assert!(buffer.as_ptr_slice().subslice(4096..8192).iter_as::<u8>().all(|b| b == 0x00));
        assert!(buffer.as_ptr_slice().subslice(8192..12288).iter_as::<u8>().all(|b| b == 0x22));
    }

    #[test]
    fn test_read_buffer_from_extents_sync_error_disarms_callback() {
        struct SecondChunkSyncErrorService {
            inner: FakeBlockService,
        }
        impl BlockService for SecondChunkSyncErrorService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                self.inner.allocate_buffer(max_len)
            }
            fn read_blocks(
                &self,
                device_offset: u64,
                dest_buffer: OwnedBuffer,
                on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                if device_offset == 8192 {
                    Err(anyhow!("sync failure on second chunk"))
                } else {
                    self.inner.read_blocks(device_offset, dest_buffer, on_complete)
                }
            }
        }

        let inner = FakeBlockService::new(vec![0u8; 32768]);
        let service = Arc::new(SecondChunkSyncErrorService { inner });

        // Two extents: 0..4096 (offset 0) and 4096..8192 (offset 8192).
        let mappings = Extents::try_new(
            [Extent::new(0..4096, Some(0)), Extent::new(4096..8192, Some(8192))],
            0,
        )
        .unwrap();

        let dest = service.allocate_buffer(8192);
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        let res = read_buffer_from_extents(&mappings, 0, dest, service.as_ref(), move |_res| {
            completed_clone.store(true, Ordering::Relaxed);
        });

        // The dispatch must fail synchronously with Err:
        assert!(res.is_err());
        // The callback must NOT have been invoked (and must not be invoked by chunk 1 in-flight
        // completion):
        assert!(!completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_buffer_from_extents_in_flight_chunk_succeeds_after_sync_error() {
        struct InFlightFirstChunkService {
            inner: FakeBlockService,
            pending_first_chunk:
                Mutex<Option<(OwnedBuffer, Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>)>>,
        }
        impl BlockService for InFlightFirstChunkService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                self.inner.allocate_buffer(max_len)
            }
            fn read_blocks(
                &self,
                device_offset: u64,
                dest_buffer: OwnedBuffer,
                on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                if device_offset == 0 {
                    // Keep chunk 1 in flight in the background without invoking callback yet.
                    *self.pending_first_chunk.lock() = Some((dest_buffer, on_complete));
                    Ok(())
                } else if device_offset == 8192 {
                    // Chunk 2 fails synchronously.
                    Err(anyhow!("sync failure on second chunk"))
                } else {
                    self.inner.read_blocks(device_offset, dest_buffer, on_complete)
                }
            }
        }

        let inner = FakeBlockService::new(vec![0u8; 32768]);
        let service =
            Arc::new(InFlightFirstChunkService { inner, pending_first_chunk: Mutex::new(None) });

        // Two extents: 0..4096 (offset 0) and 4096..8192 (offset 8192).
        let mappings = Extents::try_new(
            [Extent::new(0..4096, Some(0)), Extent::new(4096..8192, Some(8192))],
            0,
        )
        .unwrap();

        let dest = service.allocate_buffer(8192);
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        let res = read_buffer_from_extents(&mappings, 0, dest, service.as_ref(), move |_res| {
            completed_clone.store(true, Ordering::Relaxed);
        });

        // The dispatch must fail synchronously with Err:
        assert!(res.is_err());
        assert!(!completed.load(Ordering::Relaxed));

        // Now complete the in-flight chunk 1 with Ok after dispatch has already failed.
        let (buf, cb) = service.pending_first_chunk.lock().take().expect("chunk 1 pending");
        cb(Ok(buf));

        // The user callback must NOT have been called.
        assert!(!completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_read_buffer_from_extents_async_error_disarms_subsequent_chunks() {
        struct AsyncErrorService {
            inner: FakeBlockService,
            pending_chunks:
                Mutex<Vec<(OwnedBuffer, Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>, u64)>>,
        }
        impl BlockService for AsyncErrorService {
            fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
                self.inner.allocate_buffer(max_len)
            }
            fn read_blocks(
                &self,
                device_offset: u64,
                dest_buffer: OwnedBuffer,
                on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
            ) -> Result<(), Error> {
                self.pending_chunks.lock().push((dest_buffer, on_complete, device_offset));
                Ok(())
            }
        }

        let inner = FakeBlockService::new(vec![0u8; 32768]);
        let service = Arc::new(AsyncErrorService { inner, pending_chunks: Mutex::new(Vec::new()) });

        let mappings = Extents::try_new(
            [Extent::new(0..4096, Some(0)), Extent::new(4096..8192, Some(8192))],
            0,
        )
        .unwrap();

        let dest = service.allocate_buffer(8192);
        let err_count = Arc::new(AtomicUsize::new(0));
        let ok_count = Arc::new(AtomicUsize::new(0));
        let err_count_clone = err_count.clone();
        let ok_count_clone = ok_count.clone();

        let res = read_buffer_from_extents(&mappings, 0, dest, service.as_ref(), move |res| {
            if res.is_ok() {
                ok_count_clone.fetch_add(1, Ordering::Relaxed);
            } else {
                err_count_clone.fetch_add(1, Ordering::Relaxed);
            }
        });
        assert!(res.is_ok());

        let mut chunks = service.pending_chunks.lock();
        assert_eq!(chunks.len(), 2);
        let (buf1, cb1, _) = chunks.remove(0);
        let (buf2, cb2, _) = chunks.remove(0);
        drop(chunks);

        // Chunk 1 fails asynchronously:
        drop(buf1);
        cb1(Err(anyhow!("async failure on chunk 1")));
        assert_eq!(err_count.load(Ordering::Relaxed), 1);
        assert_eq!(ok_count.load(Ordering::Relaxed), 0);

        // Chunk 2 completes successfully afterwards:
        cb2(Ok(buf2));
        // Callback must NOT have been called again (ok_count remains 0, err_count remains 1):
        assert_eq!(err_count.load(Ordering::Relaxed), 1);
        assert_eq!(ok_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_child_block_service() {
        let mut device_data = vec![0u8; 16384];
        device_data[4096..8192].fill(0x55);
        let parent_service = Arc::new(FakeBlockService::new(device_data));

        let mappings = Extents::try_new([Extent::new(0..4096, Some(4096))], 0).unwrap();
        let file = Arc::new(File::new(mappings, 4096, Transform::None));

        let child_service = ChildBlockService::new(parent_service.clone(), file.clone());
        assert_eq!(child_service.file().extents().iter_extents(0).count(), 1);

        let dest = child_service.allocate_buffer(4096);
        let (tx, rx) = std::sync::mpsc::channel();
        child_service
            .read_blocks(
                0,
                dest,
                Box::new(move |res| {
                    tx.send(res).unwrap();
                }),
            )
            .unwrap();

        let buffer = rx.recv().unwrap().unwrap();
        assert_eq!(buffer.len(), 4096);
        assert!(buffer.as_ptr_slice().iter_as::<u8>().all(|b| b == 0x55));
    }

    #[test]
    fn test_read_buffer_from_extents_non_aligned_start() {
        let base_device_offset = 17408u64; // e.g. LBA 34 on 512-byte sector disk
        let mut device_data = vec![0u8; 32768];
        device_data[base_device_offset as usize..(base_device_offset as usize + 4096)].fill(0x77);
        let service = Arc::new(FakeBlockService::new(device_data));

        let mappings =
            Extents::try_new([Extent::new(0..4096, Some(base_device_offset))], base_device_offset)
                .unwrap();

        let dest = service.allocate_buffer(4096);
        let (tx, rx) = std::sync::mpsc::channel();
        read_buffer_from_extents(&mappings, 0, dest, service.as_ref(), move |res| {
            tx.send(res).unwrap();
        })
        .unwrap();

        let buffer = rx.recv().unwrap().unwrap();
        assert_eq!(buffer.len(), 4096);
        assert!(buffer.as_ptr_slice().iter_as::<u8>().all(|b| b == 0x77));
    }
}
