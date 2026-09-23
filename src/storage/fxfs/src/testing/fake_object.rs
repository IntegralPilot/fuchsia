// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use crate::object_store::journal::JournalHandle;
use anyhow::Error;
use async_trait::async_trait;
use fuchsia_sync::Mutex;
use std::cmp::min;
use std::ops::Range;
use std::sync::Arc;
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_units::BlockSize;

pub struct FakeObject {
    buf: Mutex<Vec<u8>>,
}

impl FakeObject {
    pub fn new() -> Self {
        FakeObject { buf: Mutex::new(Vec::new()) }
    }

    fn read(&self, offset: u64, buf: MutableBufferRef<'_>) -> Result<usize, Error> {
        let our_buf = self.buf.lock();
        let to_do = min(buf.len(), our_buf.len() - offset as usize);
        buf.subslice_mut(0..to_do)
            .copy_from_slice(&our_buf[offset as usize..offset as usize + to_do]);
        Ok(to_do)
    }

    fn write_or_append(&self, offset: Option<u64>, buf: BufferRef<'_>) -> Result<u64, Error> {
        let mut our_buf = self.buf.lock();
        let offset = offset.unwrap_or(our_buf.len() as u64);
        let required_len = offset as usize + buf.len();
        if our_buf.len() < required_len {
            our_buf.resize(required_len, 0);
        }
        buf.copy_to_slice(&mut our_buf[offset as usize..offset as usize + buf.len()]);
        Ok(our_buf.len() as u64)
    }

    fn truncate(&self, size: u64) {
        self.buf.lock().resize(size as usize, 0);
    }

    pub fn get_size(&self) -> u64 {
        self.buf.lock().len() as u64
    }
}

pub struct FakeObjectHandle {
    object: Arc<FakeObject>,
    allocator: BufferAllocator,
    block_size: BlockSize,
}

impl FakeObjectHandle {
    pub fn new_with_block_size(object: Arc<FakeObject>, block_size: BlockSize) -> Self {
        let allocator =
            BufferAllocator::new(block_size.get() as usize, BufferSource::new(32 * 1024 * 1024));
        Self { object, allocator, block_size }
    }
    pub fn new(object: Arc<FakeObject>) -> Self {
        Self::new_with_block_size(object, BlockSize::SIZE_512B)
    }
}

impl ObjectHandle for FakeObjectHandle {
    fn object_id(&self) -> u64 {
        0
    }

    fn block_size(&self) -> BlockSize {
        self.block_size
    }

    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }
}

#[async_trait]
impl ReadObjectHandle for FakeObjectHandle {
    async fn read(&self, offset: u64, buf: MutableBufferRef<'_>) -> Result<usize, Error> {
        self.object.read(offset, buf)
    }

    fn get_size(&self) -> u64 {
        self.object.get_size()
    }
}

impl WriteObjectHandle for FakeObjectHandle {
    async fn write_or_append(&self, offset: Option<u64>, buf: BufferRef<'_>) -> Result<u64, Error> {
        self.object.write_or_append(offset, buf)
    }

    async fn truncate(&self, size: u64) -> Result<(), Error> {
        self.object.truncate(size);
        Ok(())
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

impl JournalHandle for FakeObjectHandle {
    fn end_offset(&self) -> Option<u64> {
        None
    }
    fn push_extent(&mut self, _added_offset: u64, _device_range: Range<u64>) {
        // NOP
    }
    fn discard_extents(&mut self, _discard_offset: u64) {
        // NOP
    }
}
