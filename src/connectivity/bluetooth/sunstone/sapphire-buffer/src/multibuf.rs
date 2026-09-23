// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Buffer;
use sapphire_collections::deque::Deque;
use sapphire_collections::storage::StorageFamily;

/// A logical byte sequence backed by multiple non-contiguous [`Buffer`] chunks.
///
/// Chunks are queued in a [`Deque`] parameterized over a [`StorageFamily`].
pub struct MultiBuf<'a, S: StorageFamily> {
    chunks: Deque<Buffer<'a>, S>,
}

impl<'a, S: StorageFamily> MultiBuf<'a, S> {
    /// Creates a new, empty `MultiBuf` backed by the provided storage container.
    pub fn new_in(storage: S::Storage<Buffer<'a>>) -> Self {
        Self { chunks: Deque::new_in(storage) }
    }
}

impl<'a, S: StorageFamily> Default for MultiBuf<'a, S>
where
    Deque<Buffer<'a>, S>: Default,
{
    fn default() -> Self {
        Self { chunks: Default::default() }
    }
}

impl<'a, S: StorageFamily> MultiBuf<'a, S> {
    /// Creates a new, empty `MultiBuf` using the default storage initialization.
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Returns `true` if the multi-buf contains no chunks.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Returns the number of chunks currently stored in the multi-buf.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Returns the total number of active bytes across all combined chunks.
    pub fn len(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.len()).sum()
    }

    /// Returns the total underlying allocated capacity across all combined chunks.
    pub fn capacity(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.capacity()).sum()
    }

    /// Appends a new chunk to the back of the multi-buf.
    ///
    /// Returns `Err(chunk)` if the backing deque is at full capacity.
    pub fn push_back(&mut self, chunk: Buffer<'a>) -> Result<(), Buffer<'a>> {
        self.chunks.push_back(chunk)
    }

    /// Prepends a new chunk to the front of the multi-buf.
    ///
    /// Returns `Err(chunk)` if the backing deque is at full capacity.
    pub fn push_front(&mut self, chunk: Buffer<'a>) -> Result<(), Buffer<'a>> {
        self.chunks.push_front(chunk)
    }

    /// Removes and returns the last chunk from the back of the multi-buf, if any.
    pub fn pop_back(&mut self) -> Option<Buffer<'a>> {
        self.chunks.pop_back()
    }

    /// Removes and returns the first chunk from the front of the multi-buf, if any.
    pub fn pop_front(&mut self) -> Option<Buffer<'a>> {
        self.chunks.pop_front()
    }

    /// Discards `offset` bytes from the front of the entire multi-buf sequence.
    ///
    /// If the front chunks are fully consumed by the offset, they are popped and dropped.
    /// If a chunk is partially consumed, it is truncated in place.
    pub fn truncate_front(&mut self, mut offset: usize) {
        while offset > 0 {
            if let Some(mut front) = self.chunks.pop_front() {
                let chunk_len = front.len();
                if offset >= chunk_len {
                    offset -= chunk_len;
                } else {
                    front.truncate_front(offset).unwrap();
                    self.chunks
                        .push_front(front)
                        .expect("re-pushing popped front chunk cannot exceed capacity");
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// Discards `size` bytes from the back of the entire multi-buf sequence.
    ///
    /// If the trailing chunks are fully consumed by the size, they are popped and dropped.
    /// If a chunk is partially consumed, it is truncated from the back in place.
    pub fn truncate_back(&mut self, mut size: usize) {
        while size > 0 {
            if let Some(mut back) = self.chunks.pop_back() {
                let chunk_len = back.len();
                if size >= chunk_len {
                    size -= chunk_len;
                } else {
                    back.truncate_back(size).unwrap();
                    self.chunks
                        .push_back(back)
                        .expect("re-pushing popped back chunk cannot exceed capacity");
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// Copies bytes from the multi-buf into the destination buffer, starting at the specified
    /// logical `offset`.
    ///
    /// Returns the total number of bytes copied.
    pub fn copy_to(&self, dst: &mut [u8], mut offset: usize) -> usize {
        if dst.is_empty() {
            return 0;
        }
        let mut dst_offset = 0;

        for chunk in self.chunks.iter() {
            let chunk_len = chunk.len();
            if offset >= chunk_len {
                offset -= chunk_len;
                continue;
            }

            let chunk_slice = chunk.as_slice();
            let chunk_avail = &chunk_slice[offset..];
            offset = 0;

            let to_copy = core::cmp::min(chunk_avail.len(), dst.len() - dst_offset);
            dst[dst_offset..dst_offset + to_copy].copy_from_slice(&chunk_avail[..to_copy]);
            dst_offset += to_copy;

            if dst_offset >= dst.len() {
                break;
            }
        }
        dst_offset
    }

    /// Copies bytes from the source buffer into the multi-buf, starting at the specified
    /// logical `offset`.
    ///
    /// Returns the total number of bytes copied into the multi-buf.
    pub fn copy_from(&mut self, src: &[u8], mut offset: usize) -> usize {
        if src.is_empty() {
            return 0;
        }
        let mut src_offset = 0;

        for chunk in self.chunks.iter_mut() {
            let chunk_len = chunk.len();
            if offset >= chunk_len {
                offset -= chunk_len;
                continue;
            }

            let chunk_slice = chunk.as_mut();
            let chunk_avail = &mut chunk_slice[offset..];
            offset = 0;

            let to_copy = core::cmp::min(chunk_avail.len(), src.len() - src_offset);
            chunk_avail[..to_copy].copy_from_slice(&src[src_offset..src_offset + to_copy]);
            src_offset += to_copy;

            if src_offset >= src.len() {
                break;
            }
        }
        src_offset
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use crate::allocated::StdVecBufferProvider;
    use sapphire_collections::storage::storages::ArrayStorage;

    #[test]
    fn test_multibuf_basic_ops() {
        let provider = StdVecBufferProvider::default();

        let mut buf1 = provider.acquire_buffer(10);
        let mut buf2 = provider.acquire_buffer(20);
        let mut buf3 = provider.acquire_buffer(30);

        buf1.as_mut().fill(1);
        buf2.as_mut().fill(2);
        buf3.as_mut().fill(3);

        let mut multibuf = MultiBuf::<ArrayStorage<4>>::new();
        assert!(multibuf.is_empty());
        assert_eq!(multibuf.chunk_count(), 0);
        assert_eq!(multibuf.len(), 0);

        multibuf.push_back(buf1).unwrap();
        multibuf.push_back(buf2).unwrap();
        multibuf.push_back(buf3).unwrap();

        assert_eq!(multibuf.chunk_count(), 3);
        assert_eq!(multibuf.len(), 60);

        // Copy out and verify
        let mut out = [0u8; 60];
        let copied = multibuf.copy_to(&mut out, 0);
        assert_eq!(copied, 60);
        assert_eq!(&out[0..10], &[1u8; 10]);
        assert_eq!(&out[10..30], &[2u8; 20]);
        assert_eq!(&out[30..60], &[3u8; 30]);

        // Copy from and verify modification
        let src = [9u8; 15];
        let written = multibuf.copy_from(&src, 5);
        assert_eq!(written, 15);

        let mut out_modified = [0u8; 60];
        multibuf.copy_to(&mut out_modified, 0);
        assert_eq!(&out_modified[0..5], &[1u8; 5]);
        assert_eq!(&out_modified[5..20], &[9u8; 15]);
        assert_eq!(&out_modified[20..30], &[2u8; 10]);
        assert_eq!(&out_modified[30..60], &[3u8; 30]);

        // Truncate front
        multibuf.truncate_front(15);
        assert_eq!(multibuf.chunk_count(), 2);
        assert_eq!(multibuf.len(), 45);

        let mut out2 = [0u8; 45];
        multibuf.copy_to(&mut out2, 0);
        let mut expected = [2u8; 15];
        expected[0..5].fill(9);
        assert_eq!(&out2[0..15], &expected); // The remainder of buf2 after stripping 5 (buf1) + 10 (buf2)
        assert_eq!(&out2[15..45], &[3u8; 30]);

        // Truncate back by 20 bytes (leaving 25 bytes out of 45)
        multibuf.truncate_back(20);
        assert_eq!(multibuf.chunk_count(), 2);
        assert_eq!(multibuf.len(), 25);

        let mut out3 = [0u8; 25];
        multibuf.copy_to(&mut out3, 0);
        assert_eq!(&out3[0..15], &expected);
        assert_eq!(&out3[15..25], &[3u8; 10]);
    }

    #[test]
    fn test_multibuf_push_front_and_pop() {
        let provider = StdVecBufferProvider::default();

        let mut buf1 = provider.acquire_buffer(10);
        let mut buf2 = provider.acquire_buffer(20);

        buf1.as_mut().fill(1);
        buf2.as_mut().fill(2);

        let mut multibuf = MultiBuf::<ArrayStorage<4>>::new();
        multibuf.push_back(buf2).unwrap();
        multibuf.push_front(buf1).unwrap();

        assert_eq!(multibuf.chunk_count(), 2);
        assert_eq!(multibuf.len(), 30);

        let mut out = [0u8; 30];
        multibuf.copy_to(&mut out, 0);
        assert_eq!(&out[0..10], &[1u8; 10]);
        assert_eq!(&out[10..30], &[2u8; 20]);

        let popped_front = multibuf.pop_front().unwrap();
        assert_eq!(popped_front.capacity(), 10);
        assert_eq!(multibuf.len(), 20);

        let popped_back = multibuf.pop_back().unwrap();
        assert_eq!(popped_back.capacity(), 20);
        assert_eq!(multibuf.len(), 0);
        assert!(multibuf.is_empty());
    }
}

#[cfg(all(test, feature = "std"))]
mod proptests {
    extern crate alloc;
    use super::*;
    use crate::allocated::StdVecBufferProvider;
    use proptest::prelude::*;
    use sapphire_collections::storage::storages::Global;

    #[derive(Debug, Clone)]
    enum MultiBufOp {
        PushBack { allocator_idx: usize, size: usize },
        PushFront { allocator_idx: usize, size: usize },
        PopBack,
        PopFront,
        TruncateFront(usize),
        TruncateBack(usize),
        Write { offset: usize, val: u8, len: usize },
    }

    fn op_strategy() -> impl Strategy<Value = MultiBufOp> {
        prop_oneof![
            (0usize..3, 1usize..32)
                .prop_map(|(allocator_idx, size)| MultiBufOp::PushBack { allocator_idx, size }),
            (0usize..3, 1usize..32)
                .prop_map(|(allocator_idx, size)| MultiBufOp::PushFront { allocator_idx, size }),
            Just(MultiBufOp::PopBack),
            Just(MultiBufOp::PopFront),
            (0usize..64).prop_map(MultiBufOp::TruncateFront),
            (0usize..128).prop_map(MultiBufOp::TruncateBack),
            (0usize..128, 0u8..255, 1usize..32).prop_map(|(offset, val, len)| MultiBufOp::Write {
                offset,
                val,
                len
            }),
        ]
    }

    proptest! {
        #[test]
        fn test_multibuf_differential_heterogeneous(
            ops in prop::collection::vec(op_strategy(), 1..100),
        ) {
            let alloc0 = StdVecBufferProvider::default();
            let alloc1 = StdVecBufferProvider::default();
            let alloc2 = StdVecBufferProvider::default();

            let mut multibuf = MultiBuf::<Global>::new();
            let mut reference = alloc::vec::Vec::<u8>::new();
            let mut chunk_counter = 0usize;

            for op in ops {
                match op {
                    MultiBufOp::PushBack { allocator_idx, size } => {
                        let mut chunk = match allocator_idx {
                            0 => alloc0.acquire_buffer(size),
                            1 => alloc1.acquire_buffer(size),
                            _ => alloc2.acquire_buffer(size),
                        };
                        let chunk_val = (chunk_counter % 256) as u8;
                        chunk_counter += 1;
                        chunk.as_mut().fill(chunk_val);

                        if multibuf.push_back(chunk).is_ok() {
                            reference.extend(core::iter::repeat(chunk_val).take(size));
                        }
                    }
                    MultiBufOp::PushFront { allocator_idx, size } => {
                        let mut chunk = match allocator_idx {
                            0 => alloc0.acquire_buffer(size),
                            1 => alloc1.acquire_buffer(size),
                            _ => alloc2.acquire_buffer(size),
                        };
                        let chunk_val = (chunk_counter % 256) as u8;
                        chunk_counter += 1;
                        chunk.as_mut().fill(chunk_val);

                        if multibuf.push_front(chunk).is_ok() {
                            let mut new_ref = alloc::vec::Vec::new();
                            new_ref.extend(core::iter::repeat(chunk_val).take(size));
                            new_ref.extend(&reference);
                            reference = new_ref;
                        }
                    }
                    MultiBufOp::PopBack => {
                        if let Some(popped) = multibuf.pop_back() {
                            let popped_len = popped.len();
                            reference.truncate(reference.len().saturating_sub(popped_len));
                        }
                    }
                    MultiBufOp::PopFront => {
                            if let Some(popped) = multibuf.pop_front() {
                                let popped_len = popped.len();
                                if popped_len >= reference.len() {
                                    reference.clear();
                                } else {
                                    reference.drain(0..popped_len);
                                }
                            }
                        }
                        MultiBufOp::TruncateFront(amt) => {
                            multibuf.truncate_front(amt);
                            if amt >= reference.len() {
                                reference.clear();
                            } else {
                                reference.drain(0..amt);
                            }
                        }
                        MultiBufOp::TruncateBack(amt) => {
                            multibuf.truncate_back(amt);
                            let new_len = reference.len().saturating_sub(amt);
                            reference.truncate(new_len);
                        }
                        MultiBufOp::Write { offset, val, len } => {
                            if offset < reference.len() {
                                let src = alloc::vec![val; len];
                                let written = multibuf.copy_from(&src, offset);
                                let write_end = core::cmp::min(offset + written, reference.len());
                                if write_end > offset {
                                    reference[offset..write_end].fill(val);
                                }
                            }
                        }
                    }

                    // Assert equality after every operation
                    assert_eq!(multibuf.len(), reference.len());
                    let mut out = alloc::vec![0u8; reference.len()];
                    multibuf.copy_to(&mut out, 0);
                    assert_eq!(out, reference);
                }
        }
    }
}
