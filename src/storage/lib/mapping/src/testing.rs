// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{DeliveryHandler, PageRequest};
use delivery_blob::compression::{ChunkedArchiveError, DataBuffer};
use fuchsia_sync::Mutex;
use std::ops::{Deref, DerefMut, Range};
use std::sync::Arc;
use storage_ptr_slice::MutPtrByteSlice;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[derive(Clone, Copy, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(C, align(64))]
struct Block([u8; 64]);

impl Default for Block {
    fn default() -> Self {
        Self([0u8; 64])
    }
}

/// A byte buffer aligned to 64 bytes, ensuring compatibility with cryptographic and DMA operations.
#[derive(Clone, Default)]
pub struct AlignedBuffer {
    blocks: Vec<Block>,
    len: usize,
}

impl AlignedBuffer {
    pub fn new(len: usize) -> Self {
        let block_count = len.div_ceil(64);
        Self { blocks: vec![Block::default(); block_count], len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn resize(&mut self, new_len: usize, val: u8) {
        let block_count = new_len.div_ceil(64);
        if block_count > self.blocks.len() {
            self.blocks.resize(block_count, Block::default());
        } else {
            self.blocks.truncate(block_count);
        }
        if new_len > self.len {
            self.blocks.as_mut_bytes()[self.len..new_len].fill(val);
        }
        self.len = new_len;
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.blocks.as_bytes()[..self.len]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.blocks.as_mut_bytes()[..self.len]
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

impl Deref for AlignedBuffer {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for AlignedBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

#[derive(Default)]
pub struct TestVecBufferInner {
    pub commits: Vec<(u64, usize)>,
    pub output: Vec<u8>,
}

#[derive(Clone)]
pub struct TestVecBufferReceiver(pub Arc<Mutex<TestVecBufferInner>>);

impl TestVecBufferReceiver {
    pub fn commits(&self) -> Vec<(u64, usize)> {
        self.0.lock().commits.clone()
    }

    pub fn output(&self) -> Vec<u8> {
        self.0.lock().output.clone()
    }
}

pub struct TestVecBuffer {
    pub data: AlignedBuffer,
    pub range: Range<u64>,
    pub committed_len: usize,
    pub offset: u64,
    pub receiver: TestVecBufferReceiver,
}

impl TestVecBuffer {
    pub fn new(size: usize) -> (Self, TestVecBufferReceiver) {
        Self::new_with_offset(size, 0)
    }

    pub fn new_with_offset(size: usize, offset: u64) -> (Self, TestVecBufferReceiver) {
        let receiver = TestVecBufferReceiver(Arc::new(Mutex::new(TestVecBufferInner::default())));
        let range = offset..offset + size as u64;
        let buf = Self {
            data: AlignedBuffer::new(size),
            range,
            committed_len: 0,
            offset,
            receiver: receiver.clone(),
        };
        (buf, receiver)
    }

    pub fn new_with_range(range: Range<u64>) -> (Self, TestVecBufferReceiver) {
        let size = (range.end - range.start) as usize;
        let receiver = TestVecBufferReceiver(Arc::new(Mutex::new(TestVecBufferInner::default())));
        let buf = Self {
            data: AlignedBuffer::new(size),
            offset: range.start,
            range,
            committed_len: 0,
            receiver: receiver.clone(),
        };
        (buf, receiver)
    }

    pub fn new_unprepared() -> (Self, TestVecBufferReceiver) {
        Self::new_unprepared_with_range(0..0)
    }

    pub fn new_unprepared_with_range(range: Range<u64>) -> (Self, TestVecBufferReceiver) {
        let receiver = TestVecBufferReceiver(Arc::new(Mutex::new(TestVecBufferInner::default())));
        let buf = Self {
            data: AlignedBuffer::default(),
            range,
            committed_len: 0,
            offset: 0,
            receiver: receiver.clone(),
        };
        (buf, receiver)
    }
}

impl Drop for TestVecBuffer {
    fn drop(&mut self) {
        self.receiver.0.lock().output = self.data.to_vec();
    }
}

impl DataBuffer for TestVecBuffer {
    fn range(&self) -> Range<u64> {
        self.range.clone()
    }

    fn mut_ptr_slice(&mut self) -> MutPtrByteSlice<'_> {
        let remaining = &mut self.data[self.committed_len..];
        MutPtrByteSlice::from(remaining)
    }

    fn commit(&mut self, size: usize) -> Result<(), ChunkedArchiveError> {
        self.receiver.0.lock().commits.push((self.offset, size));
        self.offset += size as u64;
        self.committed_len += size;
        Ok(())
    }
}

impl PageRequest for TestVecBuffer {
    fn prepare(&mut self, read_range: Range<u64>) -> Result<(), ChunkedArchiveError> {
        let size = (read_range.end - read_range.start) as usize;
        if self.data.len() < size {
            self.data.resize(size, 0);
        }
        self.range = read_range.clone();
        self.offset = read_range.start;
        Ok(())
    }
}

/// A [`DeliveryHandler`] test adapter that implements `get_page_request` via a closure.
///
/// The closure takes `(key: u64, range: Range<u64>)` where:
/// - `key`: The pager port key identifying the mapped file.
/// - `range`: The byte range requested to be paged in.
///
/// And returns `R: PageRequest` (any type implementing [`PageRequest`]) to receive the paged data.
///
/// [`DeliveryHandler::register_blob`] defaults to a no-op `Ok(())`.
pub struct TestDeliveryHandler<F>(pub F);

impl<F, R: PageRequest> DeliveryHandler for TestDeliveryHandler<F>
where
    F: Fn(u64, Range<u64>) -> R + Send + Sync + 'static,
{
    type Request = R;

    fn get_page_request(self: &Arc<Self>, key: u64, range: Range<u64>) -> Self::Request {
        (self.0)(key, range)
    }
}
