// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::compression::{ChunkedArchiveError, CompressionAlgorithm, ThreadLocalDecompressor};
use std::borrow::Borrow;
use std::ops::Range;
use storage_ptr_slice::{MutPtrByteSlice, PtrByteSlice};

/// Trait for destination buffers where uncompressed or decompressed blob data is written.
pub trait DataBuffer: Send + 'static {
    /// Returns the logical byte range for this buffer.
    fn range(&self) -> Range<u64>;

    /// Returns a raw pointer slice to the remaining uncommitted memory in this allocation.
    fn mut_ptr_slice(&mut self) -> MutPtrByteSlice<'_>;

    /// Incrementally commits `size` bytes of data within this allocation, advancing the start
    /// of the remaining memory returned by subsequent calls to [`mut_ptr_slice`].
    fn commit(&mut self, size: usize) -> Result<(), ChunkedArchiveError>;
}

pub struct CompressionInfo {
    chunk_size: u64,
    compressed_size: u64,
    // The chunked compression format stores 0 as the first offset but it's not stored here. Not
    // storing the 0 avoids the allocation for blobs smaller than the chunk size.
    small_offsets: Box<[u32]>,
    large_offsets: Box<[u64]>,
    decompressor: ThreadLocalDecompressor,
}

impl CompressionInfo {
    pub fn new(
        chunk_size: u64,
        compressed_size: u64,
        offsets: &[u64],
        compression_algorithm: CompressionAlgorithm,
    ) -> Result<Self, ChunkedArchiveError> {
        let decompressor = compression_algorithm.thread_local_decompressor();
        if chunk_size == 0 {
            return Err(ChunkedArchiveError::IntegrityError);
        } else if offsets.is_empty() || *offsets.first().unwrap() != 0 {
            // There should always be at least 1 offset and the first offset must always be 0.
            return Err(ChunkedArchiveError::IntegrityError);
        } else if !offsets.array_windows().all(|[a, b]| a < b) {
            // The offsets must be in ascending order.
            return Err(ChunkedArchiveError::IntegrityError);
        } else if offsets.len() == 1 {
            // Simple case where the blob is smaller than the chunk size so only the 0 offset is
            // present. The 0 isn't stored so no allocation is necessary.
            Ok(Self {
                chunk_size,
                compressed_size,
                small_offsets: Box::default(),
                large_offsets: Box::default(),
                decompressor,
            })
        } else if *offsets.last().unwrap() <= u32::MAX as u64 {
            // Check the last index first since most compressed blobs are going to be smaller
            // than 4GiB making all offsets small.
            Ok(Self {
                chunk_size,
                compressed_size,
                small_offsets: offsets[1..].iter().map(|x| *x as u32).collect(),
                large_offsets: Box::default(),
                decompressor,
            })
        } else {
            // The partition point is the index of the first compressed offset that's > u32::MAX.
            let partition_point = offsets.partition_point(|&x| x <= u32::MAX as u64);
            Ok(Self {
                chunk_size,
                compressed_size,
                small_offsets: offsets[1..partition_point].iter().map(|x| *x as u32).collect(),
                large_offsets: offsets[partition_point..].into(),
                decompressor,
            })
        }
    }

    /// Returns the chunk size for this compressed blob.
    pub fn chunk_size(&self) -> u64 {
        self.chunk_size
    }

    /// Returns the total compressed size of this blob.
    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    /// Returns the compressed range for the specified uncompressed range.
    pub fn compressed_range_for_uncompressed_range(
        &self,
        range: &Range<u64>,
    ) -> Result<Range<u64>, ChunkedArchiveError> {
        if range.start % self.chunk_size != 0 || range.start >= range.end {
            return Err(ChunkedArchiveError::IntegrityError);
        }

        let start_chunk_index = (range.start / self.chunk_size) as usize;
        let start_offset = self
            .compressed_offset_for_chunk_index(start_chunk_index)
            .ok_or(ChunkedArchiveError::OutOfRange)?;

        // The end of the range may not be aligned to the chunk size for the last chunk.
        let end_chunk_index = range.end.div_ceil(self.chunk_size) as usize;
        let end_offset = match self.compressed_offset_for_chunk_index(end_chunk_index) {
            None => self.compressed_size,
            Some(offset) => {
                // This isn't the last chunk so the end must be aligned.
                if !range.end.is_multiple_of(self.chunk_size) {
                    return Err(ChunkedArchiveError::IntegrityError);
                }
                // `CompressionInfo::new` validates that all of the offsets are ascending.
                offset
            }
        };

        Ok(start_offset..end_offset)
    }

    fn compressed_offset_for_chunk_index(&self, chunk_index: usize) -> Option<u64> {
        if chunk_index == 0 {
            Some(0)
        } else if chunk_index - 1 < self.small_offsets.len() {
            Some(self.small_offsets[chunk_index - 1] as u64)
        } else if chunk_index - 1 - self.small_offsets.len() < self.large_offsets.len() {
            Some(self.large_offsets[chunk_index - 1 - self.small_offsets.len()])
        } else {
            None
        }
    }

    /// Decompress the bytes of `src` into `dst`.
    ///   - `src` is allowed to span multiple chunks.
    ///   - `dst` must have the exact size of the uncompressed bytes.
    ///   - `dst_start_offset` is the location of the uncompressed bytes within the blob and must be
    ///     chunk aligned. This is necessary for determining the chunk boundaries in `src`.
    pub fn decompress<'a>(
        &self,
        src: impl Into<PtrByteSlice<'a>>,
        mut dst: &mut [u8],
        dst_start_offset: u64,
    ) -> Result<(), ChunkedArchiveError> {
        let mut src = src.into();
        if dst_start_offset % self.chunk_size != 0 {
            return Err(ChunkedArchiveError::IntegrityError);
        }

        let start_chunk_index = (dst_start_offset / self.chunk_size) as usize;
        let chunk_count = dst.len().div_ceil(self.chunk_size as usize);
        let mut start_offset = self
            .compressed_offset_for_chunk_index(start_chunk_index)
            .ok_or(ChunkedArchiveError::IntegrityError)?;

        // Decompress each chunk individually.
        for chunk_index in start_chunk_index..(start_chunk_index + chunk_count) {
            match self.compressed_offset_for_chunk_index(chunk_index + 1) {
                Some(end_offset) => {
                    let len = (end_offset - start_offset) as usize;
                    if len > src.len() {
                        return Err(ChunkedArchiveError::IntegrityError);
                    }
                    let (to_decompress, src_remaining) = src.split_at(len);
                    let (to_decompress_into, dst_remaining) = dst
                        .split_at_mut_checked(self.chunk_size as usize)
                        .ok_or(ChunkedArchiveError::IntegrityError)?;

                    let decompressed_bytes = self.decompressor.decompress_into(
                        to_decompress,
                        to_decompress_into,
                        chunk_index,
                    )?;
                    if decompressed_bytes != to_decompress_into.len() {
                        return Err(ChunkedArchiveError::IntegrityError);
                    }
                    src = src_remaining;
                    dst = dst_remaining;
                    start_offset = end_offset;
                }
                None => {
                    let decompressed_bytes =
                        self.decompressor.decompress_into(src, dst, chunk_index)?;
                    if decompressed_bytes != dst.len() {
                        return Err(ChunkedArchiveError::IntegrityError);
                    }
                }
            }
        }

        Ok(())
    }
}

/// Stateful streaming decompressor that receives compressed block buffers
/// and decompresses complete chunks into `page_request`.
pub struct StreamingDecompressor<C, B> {
    /// Reference or owned container for blob compression metadata.
    info: C,

    /// Target destination implementing `DataBuffer`.
    data_buffer: B,

    /// Uncompressed logical byte range remaining to be decompressed.
    range: Range<u64>,

    /// Total uncompressed size of the blob.
    uncompressed_size: u64,

    /// Index of the chunk currently being decompressed.
    chunk_index: usize,

    /// Accumulates compressed bytes for chunks that straddle buffer boundaries.
    accumulator: Vec<u8>,

    /// The current compressed device byte offset expected for incoming buffers.
    current_compressed_offset: u64,

    /// Indicates if an error occurred during decompression. Once `true`, `push` fuses and returns
    /// error.
    failed: bool,
}

impl<C: Borrow<CompressionInfo>, B: DataBuffer> StreamingDecompressor<C, B> {
    /// Creates a new streaming decompressor for `data_buffer`.
    ///
    /// Accepts any container `info` implementing `Borrow<CompressionInfo>` (e.g.
    /// `&CompressionInfo` or `Arc<CompressionInfo>`).
    /// Returns the decompressor and the block-aligned compressed byte range (`Range<u64>`) to
    /// read from storage.
    ///
    /// `data_buffer.range().start` must be chunk aligned (a multiple of `chunk_size`).
    pub fn new(
        info: C,
        uncompressed_size: u64,
        data_buffer: B,
    ) -> Result<(Self, Range<u64>), ChunkedArchiveError> {
        const BLOCK_SIZE: u64 = 4096;
        let range = data_buffer.range();
        let compressed = info.borrow().compressed_range_for_uncompressed_range(&range)?;
        let aligned = (compressed.start / BLOCK_SIZE) * BLOCK_SIZE
            ..compressed.end.next_multiple_of(BLOCK_SIZE);

        let chunk_size = info.borrow().chunk_size();
        assert_eq!(range.start % chunk_size, 0, "range.start must be chunk aligned");
        let chunk_index = (range.start / chunk_size) as usize;

        let decompressor = StreamingDecompressor {
            info,
            data_buffer,
            range,
            uncompressed_size,
            chunk_index,
            accumulator: Vec::new(),
            current_compressed_offset: aligned.start,
            failed: false,
        };

        Ok((decompressor, aligned))
    }

    /// Pushes a newly read compressed block buffer slice and decompresses any complete chunks.
    /// Fuses on error: if an error occurs or has previously occurred, returns `Err`.
    pub fn push<'b>(
        &mut self,
        buffer_slice: impl Into<PtrByteSlice<'b>>,
    ) -> Result<(), ChunkedArchiveError> {
        let buffer_slice = buffer_slice.into();
        if self.failed {
            return Err(ChunkedArchiveError::IntegrityError);
        }

        if self.range.is_empty() {
            return Ok(());
        }

        let buffer = self.current_compressed_offset
            ..self.current_compressed_offset + buffer_slice.len() as u64;
        self.current_compressed_offset = buffer.end;

        let info = self.info.borrow();
        let chunk_size = info.chunk_size();

        while self.range.start < self.range.end {
            let chunk_start = info
                .compressed_offset_for_chunk_index(self.chunk_index)
                .ok_or(ChunkedArchiveError::OutOfRange)?;
            let chunk_end = info
                .compressed_offset_for_chunk_index(self.chunk_index + 1)
                .unwrap_or_else(|| info.compressed_size());
            let chunk = chunk_start..chunk_end;

            let decompress_chunk = |compressed_src: PtrByteSlice<'_>,
                                    data_buffer: &mut B|
             -> Result<(), ChunkedArchiveError> {
                let buffer_len =
                    std::cmp::min(data_buffer.mut_ptr_slice().len(), chunk_size as usize);
                let mut dest_buffer = data_buffer.mut_ptr_slice().subslice_mut(0..buffer_len);
                let remaining = (self.uncompressed_size.saturating_sub(self.range.start)) as usize;
                let chunk_uncompressed_len = if remaining < chunk_size as usize {
                    // Zero the block tail if this partial final chunk is smaller than chunk_size.
                    let (head, mut tail) = dest_buffer.split_at_mut(remaining);
                    tail.fill(0);
                    dest_buffer = head;
                    remaining
                } else {
                    chunk_size as usize
                };

                // SAFETY: `data_buffer` is exclusively held by this decompressor, and
                // `dest_buffer` points to uncommitted remaining memory of length
                // `chunk_uncompressed_len`.
                let dst_slice = unsafe { &mut *dest_buffer.as_raw_mut_slice_ptr() };

                let decompressed_bytes = info.decompressor.decompress_into(
                    compressed_src,
                    dst_slice,
                    self.chunk_index,
                )?;
                if decompressed_bytes != chunk_uncompressed_len {
                    return Err(ChunkedArchiveError::IntegrityError);
                }
                data_buffer.commit(buffer_len)?;
                Ok(())
            };

            if chunk.start < buffer.start {
                // Chunk started in a previous buffer; accumulate remainder and decompress if
                // complete.
                assert!(!self.accumulator.is_empty());
                if chunk.end <= buffer.end {
                    let needed = (chunk.end - buffer.start) as usize;
                    buffer_slice.subslice(0..needed).append_to(&mut self.accumulator);
                    let src = self.accumulator.as_slice().into();
                    if let Err(e) = decompress_chunk(src, &mut self.data_buffer) {
                        self.failed = true;
                        return Err(e);
                    }
                    self.accumulator.clear();
                    self.range.start += chunk_size;
                    self.chunk_index += 1;
                    continue;
                } else {
                    buffer_slice.append_to(&mut self.accumulator);
                    break;
                }
            } else if chunk.end <= buffer.end {
                // Chunk is fully contained in current buffer.
                let rel_start = (chunk.start - buffer.start) as usize;
                let rel_end = (chunk.end - buffer.start) as usize;
                let compressed_slice = buffer_slice.subslice(rel_start..rel_end);

                if let Err(e) = decompress_chunk(compressed_slice, &mut self.data_buffer) {
                    self.failed = true;
                    return Err(e);
                }
                self.range.start += chunk_size;
                self.chunk_index += 1;
            } else {
                // Chunk extends past current buffer; accumulate prefix and await next buffer.
                let rel_start = (chunk.start - buffer.start) as usize;
                buffer_slice
                    .subslice(rel_start..buffer_slice.len())
                    .append_to(&mut self.accumulator);
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_info_new_small_and_large_offsets() {
        let info = CompressionInfo::new(4096, 50, &[0], CompressionAlgorithm::Zstd).unwrap();
        assert_eq!(info.chunk_size(), 4096);
        assert_eq!(info.compressed_size(), 50);
        assert_eq!(info.compressed_offset_for_chunk_index(0), Some(0));
        assert_eq!(info.compressed_offset_for_chunk_index(1), None);

        let info =
            CompressionInfo::new(4096, 350, &[0, 100, 250], CompressionAlgorithm::Zstd).unwrap();
        assert_eq!(info.compressed_offset_for_chunk_index(0), Some(0));
        assert_eq!(info.compressed_offset_for_chunk_index(1), Some(100));
        assert_eq!(info.compressed_offset_for_chunk_index(2), Some(250));
        assert_eq!(info.compressed_offset_for_chunk_index(3), None);

        let large_val = u32::MAX as u64 + 1000;
        let info = CompressionInfo::new(
            4096,
            large_val + 500,
            &[0, 500, large_val],
            CompressionAlgorithm::Zstd,
        )
        .unwrap();
        assert_eq!(info.compressed_offset_for_chunk_index(0), Some(0));
        assert_eq!(info.compressed_offset_for_chunk_index(1), Some(500));
        assert_eq!(info.compressed_offset_for_chunk_index(2), Some(large_val));
        assert_eq!(info.compressed_offset_for_chunk_index(3), None);
    }

    #[test]
    fn test_compressed_range_for_uncompressed_range() {
        let info = CompressionInfo::new(4096, 500, &[0, 100, 250, 400], CompressionAlgorithm::Zstd)
            .unwrap();
        let range = info.compressed_range_for_uncompressed_range(&(0..4096)).unwrap();
        assert_eq!(range, 0..100);

        let range = info.compressed_range_for_uncompressed_range(&(4096..12288)).unwrap();
        assert_eq!(range, 100..400);

        let range = info.compressed_range_for_uncompressed_range(&(4096..16384)).unwrap();
        assert_eq!(range, 100..500);
    }

    #[test]
    fn test_compression_info_offsets_must_start_with_zero() {
        assert!(CompressionInfo::new(4096, 100, &[], CompressionAlgorithm::Zstd).is_err());
        assert!(CompressionInfo::new(4096, 100, &[1], CompressionAlgorithm::Zstd).is_err());
        assert!(CompressionInfo::new(4096, 100, &[0], CompressionAlgorithm::Zstd).is_ok());
    }

    #[test]
    fn test_compression_info_offsets_must_be_sorted() {
        assert!(CompressionInfo::new(4096, 100, &[0, 1, 2], CompressionAlgorithm::Zstd).is_ok());
        assert!(CompressionInfo::new(4096, 100, &[0, 2, 1], CompressionAlgorithm::Zstd).is_err());
        assert!(CompressionInfo::new(4096, 100, &[0, 1, 1], CompressionAlgorithm::Zstd).is_err());
    }

    #[test]
    fn test_compression_info_splitting_offsets() {
        const MAX_SMALL_OFFSET: u64 = u32::MAX as u64;
        let compression_info =
            CompressionInfo::new(4096, 100, &[0], CompressionAlgorithm::Zstd).unwrap();
        assert!(compression_info.small_offsets.is_empty());
        assert!(compression_info.large_offsets.is_empty());

        let compression_info =
            CompressionInfo::new(4096, 20, &[0, 10], CompressionAlgorithm::Zstd).unwrap();
        assert_eq!(&*compression_info.small_offsets, &[10]);
        assert!(compression_info.large_offsets.is_empty());

        let compression_info =
            CompressionInfo::new(4096, 40, &[0, 10, 20, 30], CompressionAlgorithm::Zstd).unwrap();
        assert_eq!(&*compression_info.small_offsets, &[10, 20, 30]);
        assert!(compression_info.large_offsets.is_empty());

        let compression_info = CompressionInfo::new(
            4096,
            MAX_SMALL_OFFSET,
            &[0, MAX_SMALL_OFFSET - 1],
            CompressionAlgorithm::Zstd,
        )
        .unwrap();
        assert_eq!(&*compression_info.small_offsets, &[u32::MAX - 1]);
        assert!(compression_info.large_offsets.is_empty());

        let compression_info = CompressionInfo::new(
            4096,
            MAX_SMALL_OFFSET + 1,
            &[0, MAX_SMALL_OFFSET],
            CompressionAlgorithm::Zstd,
        )
        .unwrap();
        assert_eq!(&*compression_info.small_offsets, &[u32::MAX]);
        assert!(compression_info.large_offsets.is_empty());

        let compression_info = CompressionInfo::new(
            4096,
            MAX_SMALL_OFFSET + 2,
            &[0, MAX_SMALL_OFFSET + 1],
            CompressionAlgorithm::Zstd,
        )
        .unwrap();
        assert!(compression_info.small_offsets.is_empty());
        assert_eq!(&*compression_info.large_offsets, &[MAX_SMALL_OFFSET + 1]);

        let compression_info = CompressionInfo::new(
            4096,
            MAX_SMALL_OFFSET + 2,
            &[0, MAX_SMALL_OFFSET - 1, MAX_SMALL_OFFSET, MAX_SMALL_OFFSET + 1],
            CompressionAlgorithm::Zstd,
        )
        .unwrap();
        assert_eq!(&*compression_info.small_offsets, &[u32::MAX - 1, u32::MAX]);
        assert_eq!(&*compression_info.large_offsets, &[MAX_SMALL_OFFSET + 1]);

        let compression_info = CompressionInfo::new(
            4096,
            MAX_SMALL_OFFSET + 20,
            &[0, MAX_SMALL_OFFSET + 10],
            CompressionAlgorithm::Zstd,
        )
        .unwrap();
        assert!(compression_info.small_offsets.is_empty());
        assert_eq!(&*compression_info.large_offsets, &[MAX_SMALL_OFFSET + 10]);

        let compression_info = CompressionInfo::new(
            4096,
            MAX_SMALL_OFFSET + 30,
            &[0, MAX_SMALL_OFFSET + 10, MAX_SMALL_OFFSET + 20],
            CompressionAlgorithm::Zstd,
        )
        .unwrap();
        assert!(compression_info.small_offsets.is_empty());
        assert_eq!(
            &*compression_info.large_offsets,
            &[MAX_SMALL_OFFSET + 10, MAX_SMALL_OFFSET + 20]
        );
    }

    struct TestBuffer {
        data: Vec<u8>,
        range: Range<u64>,
        committed: usize,
    }

    impl TestBuffer {
        fn new(size: usize) -> Self {
            Self { data: vec![0u8; size], range: 0..size as u64, committed: 0 }
        }

        fn new_with_range(range: Range<u64>) -> Self {
            let size = (range.end - range.start) as usize;
            Self { data: vec![0u8; size], range, committed: 0 }
        }
    }

    impl DataBuffer for TestBuffer {
        fn range(&self) -> Range<u64> {
            self.range.clone()
        }

        fn mut_ptr_slice(&mut self) -> MutPtrByteSlice<'_> {
            let slice = &mut self.data[self.committed..];
            unsafe { MutPtrByteSlice::new(slice as *mut [u8]) }
        }

        fn commit(&mut self, size: usize) -> Result<(), ChunkedArchiveError> {
            self.committed += size;
            Ok(())
        }
    }

    #[test]
    fn test_streaming_decompressor_single_buffer() {
        let uncompressed_data: Vec<u8> = (0..32768).map(|i| (i % 251) as u8).collect();
        let options = crate::compression::ChunkedArchiveOptions::V3 {
            compression_algorithm: CompressionAlgorithm::Zstd,
        };
        let archive = crate::compression::ChunkedArchive::new(&uncompressed_data, options).unwrap();

        let mut compressed_offsets = vec![0];
        let mut compressed_data = vec![];
        for chunk in archive.chunks() {
            compressed_data.extend_from_slice(&chunk.compressed_data);
            compressed_offsets.push(compressed_data.len() as u64);
        }
        compressed_offsets.pop();

        let info = CompressionInfo::new(
            archive.chunk_size() as u64,
            compressed_data.len() as u64,
            &compressed_offsets,
            CompressionAlgorithm::Zstd,
        )
        .unwrap();

        let buf = TestBuffer::new(32768);
        let (mut decompressor, aligned) = StreamingDecompressor::new(&info, 32768, buf).unwrap();
        assert_eq!(aligned, 0..4096);

        decompressor.push(&compressed_data).unwrap();
        assert_eq!(&decompressor.data_buffer.data[..32768], &uncompressed_data[..]);
    }

    #[test]
    fn test_streaming_decompressor_straddled_buffers() {
        let uncompressed_data: Vec<u8> = (0..65536).map(|i| (i % 251) as u8).collect();
        let options = crate::compression::ChunkedArchiveOptions::V3 {
            compression_algorithm: CompressionAlgorithm::Zstd,
        };
        let archive = crate::compression::ChunkedArchive::new(&uncompressed_data, options).unwrap();

        let mut compressed_offsets = vec![0];
        let mut compressed_data = vec![];
        for chunk in archive.chunks() {
            compressed_data.extend_from_slice(&chunk.compressed_data);
            compressed_offsets.push(compressed_data.len() as u64);
        }
        compressed_offsets.pop();

        let info = CompressionInfo::new(
            archive.chunk_size() as u64,
            compressed_data.len() as u64,
            &compressed_offsets,
            CompressionAlgorithm::Zstd,
        )
        .unwrap();

        let buf = TestBuffer::new(65536);
        let (mut decompressor, _) = StreamingDecompressor::new(&info, 65536, buf).unwrap();

        for slice in compressed_data.chunks(10) {
            decompressor.push(slice).unwrap();
        }
        assert_eq!(&decompressor.data_buffer.data[..65536], &uncompressed_data[..]);
    }

    #[test]
    fn test_streaming_decompressor_partial_last_chunk_zero_tail() {
        let uncompressed_size = 32768 + 1024;
        let uncompressed_data: Vec<u8> = (0..uncompressed_size).map(|i| (i % 251) as u8).collect();

        let options = crate::compression::ChunkedArchiveOptions::V3 {
            compression_algorithm: CompressionAlgorithm::Zstd,
        };
        let archive = crate::compression::ChunkedArchive::new(&uncompressed_data, options).unwrap();

        let mut compressed_offsets = vec![0];
        let mut compressed_data = vec![];
        for chunk in archive.chunks() {
            compressed_data.extend_from_slice(&chunk.compressed_data);
            compressed_offsets.push(compressed_data.len() as u64);
        }
        compressed_offsets.pop();

        let info = CompressionInfo::new(
            archive.chunk_size() as u64,
            compressed_data.len() as u64,
            &compressed_offsets,
            CompressionAlgorithm::Zstd,
        )
        .unwrap();

        let mut buf = TestBuffer::new_with_range(0..uncompressed_size as u64);
        buf.data.resize(65536, 0);
        buf.data.fill(0xFF);

        let (mut decompressor, _) =
            StreamingDecompressor::new(&info, uncompressed_size as u64, buf).unwrap();
        decompressor.push(&compressed_data).unwrap();

        assert_eq!(&decompressor.data_buffer.data[..uncompressed_size], &uncompressed_data[..]);
        assert_eq!(&decompressor.data_buffer.data[uncompressed_size..65536], &[0u8; 31744]);
    }

    #[test]
    fn test_streaming_decompressor_unaligned_start_returns_err() {
        let info = CompressionInfo::new(4096, 500, &[0], CompressionAlgorithm::Zstd).unwrap();
        let buf = TestBuffer::new_with_range(100..4096);
        assert!(StreamingDecompressor::new(&info, 4096, buf).is_err());
    }

    #[test]
    fn test_streaming_decompressor_fused_error() {
        let info = CompressionInfo::new(4096, 500, &[0], CompressionAlgorithm::Zstd).unwrap();
        let buf = TestBuffer::new(4096);
        let (mut decompressor, _) = StreamingDecompressor::new(&info, 4096, buf).unwrap();

        let invalid_compressed_data = vec![0xFFu8; 4096];
        assert!(decompressor.push(&invalid_compressed_data).is_err());
        assert!(decompressor.push(&invalid_compressed_data).is_err());
    }
}
