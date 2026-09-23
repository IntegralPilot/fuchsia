// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use block_client::{
    BlockClient, BufferSlice, MutableBufferSlice, ReadOptions, VmoId, WriteOptions,
};
use fidl_fuchsia_storage_block as block;
use fuchsia_sync::Mutex;
use std::collections::BTreeMap;
use std::num::NonZero;
use std::ops::Range;
use std::sync::atomic::{self, AtomicU32};

type VmoRegistry = BTreeMap<u16, zx::Vmo>;

struct Inner {
    data: Vec<u8>,
    vmo_registry: VmoRegistry,
}

/// A fake instance of BlockClient for use in tests.
pub struct FakeBlockClient {
    inner: Mutex<Inner>,
    block_size: u32,
    flush_count: AtomicU32,
}

impl FakeBlockClient {
    pub fn new(block_size: u32, block_count: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                data: vec![0 as u8; block_size as usize * block_count],
                vmo_registry: BTreeMap::new(),
            }),
            block_size,
            flush_count: AtomicU32::new(0),
        }
    }

    pub fn flush_count(&self) -> u32 {
        self.flush_count.load(atomic::Ordering::Relaxed)
    }
}

impl BlockClient for FakeBlockClient {
    async unsafe fn attach_vmo(&self, vmo: &zx::Vmo) -> Result<VmoId, zx::Status> {
        let len = vmo.get_size()?;
        let vmo = vmo.create_child(zx::VmoChildOptions::SLICE, 0, len)?;
        let mut inner = self.inner.lock();
        // 0 is a sentinel value
        for id in 1..u16::MAX {
            if let std::collections::btree_map::Entry::Vacant(e) = inner.vmo_registry.entry(id) {
                e.insert(vmo);
                return Ok(VmoId::new(id));
            }
        }
        Err(zx::Status::NO_RESOURCES)
    }

    async fn detach_vmo(&self, vmo_id: VmoId) -> Result<(), zx::Status> {
        let mut inner = self.inner.lock();
        let id = vmo_id.into_id();
        if let None = inner.vmo_registry.remove(&id) { Err(zx::Status::NOT_FOUND) } else { Ok(()) }
    }

    async fn read_at_with_opts_traced(
        &self,
        buffer_slice: MutableBufferSlice<'_>,
        device_offset: u64,
        _opts: ReadOptions,
        _trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        if device_offset % self.block_size as u64 != 0 {
            return Err(zx::Status::INVALID_ARGS);
        }
        let device_offset = device_offset as usize;
        let inner = &mut *self.inner.lock();
        match buffer_slice {
            MutableBufferSlice::VmoId { vmo_id, offset, length } => {
                if offset % self.block_size as u64 != 0 {
                    return Err(zx::Status::INVALID_ARGS);
                }
                if length % self.block_size as u64 != 0 {
                    return Err(zx::Status::INVALID_ARGS);
                }
                let vmo = inner.vmo_registry.get(&vmo_id.id()).ok_or(zx::Status::INVALID_ARGS)?;
                vmo.write(&inner.data[device_offset..device_offset + length as usize], offset)?;
                Ok(())
            }
            MutableBufferSlice::Memory(slice) => {
                let len = slice.len();
                if device_offset + len > inner.data.len() {
                    return Err(zx::Status::OUT_OF_RANGE);
                }
                slice.copy_from_slice(&inner.data[device_offset..device_offset + len]);
                Ok(())
            }
        }
    }

    async fn write_at_with_opts_traced(
        &self,
        buffer_slice: BufferSlice<'_>,
        device_offset: u64,
        _opts: WriteOptions,
        _trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        if device_offset % self.block_size as u64 != 0 {
            return Err(zx::Status::INVALID_ARGS);
        }
        let device_offset = device_offset as usize;
        let inner = &mut *self.inner.lock();
        match buffer_slice {
            BufferSlice::VmoId { vmo_id, offset, length } => {
                if offset % self.block_size as u64 != 0 {
                    return Err(zx::Status::INVALID_ARGS);
                }
                if length % self.block_size as u64 != 0 {
                    return Err(zx::Status::INVALID_ARGS);
                }
                let vmo = inner.vmo_registry.get(&vmo_id.id()).ok_or(zx::Status::INVALID_ARGS)?;
                vmo.read(&mut inner.data[device_offset..device_offset + length as usize], offset)?;
                Ok(())
            }
            BufferSlice::Memory(slice) => {
                let len = slice.len();
                if device_offset + len > inner.data.len() {
                    return Err(zx::Status::OUT_OF_RANGE);
                }
                inner.data[device_offset..device_offset + len].copy_from_slice(slice);
                Ok(())
            }
        }
    }

    async fn trim_traced(&self, range: Range<u64>, _trace_flow_id: u64) -> Result<(), zx::Status> {
        if range.start % self.block_size as u64 != 0 {
            return Err(zx::Status::INVALID_ARGS);
        }
        if range.end % self.block_size as u64 != 0 {
            return Err(zx::Status::INVALID_ARGS);
        }
        // Blast over the range to simulate it being reused.
        let inner = &mut *self.inner.lock();
        if range.end as usize > inner.data.len() {
            return Err(zx::Status::OUT_OF_RANGE);
        }
        inner.data[range.start as usize..range.end as usize].fill(0xab);
        Ok(())
    }

    async fn flush_traced(&self, _trace_flow_id: u64) -> Result<(), zx::Status> {
        self.flush_count.fetch_add(1, atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn close(&self) -> Result<(), zx::Status> {
        Ok(())
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn block_count(&self) -> u64 {
        self.inner.lock().data.len() as u64 / self.block_size as u64
    }

    fn max_transfer_blocks(&self) -> Option<NonZero<u32>> {
        None
    }

    fn block_flags(&self) -> block::DeviceFlag {
        block::DeviceFlag::TRIM_SUPPORT
    }

    fn is_connected(&self) -> bool {
        true
    }
}
