// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use crate::buffer_allocator::{BufferAllocator, BufferSource};
use crate::{Device, DeviceHolder};
use anyhow::{Error, ensure};
use async_trait::async_trait;
use block_protocol::{ReadOptions, WriteFlags, WriteOptions};
use fuchsia_sync::Mutex;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

pub enum Op {
    Read,
    Write,
    Flush,
}

pub trait Observer: Send + Sync {
    fn barrier(&self) {}
}

#[derive(Debug, Default, Clone)]
struct Inner {
    data: Vec<u8>,
    blocks_written_since_last_barrier: Vec<usize>,
}

/// A Device backed by a memory buffer.
pub struct FakeDevice {
    allocator: BufferAllocator,
    inner: Mutex<Inner>,
    closed: AtomicBool,
    operation_closure: Box<dyn Fn(Op) -> Result<(), Error> + Send + Sync>,
    read_only: AtomicBool,
    poisoned: AtomicBool,
    observer: Option<Box<dyn Observer>>,
}

const TRANSFER_HEAP_SIZE: usize = 64 * 1024 * 1024;

impl FakeDevice {
    pub fn new(block_count: u64, block_size: u32) -> Self {
        let allocator =
            BufferAllocator::new(block_size as usize, BufferSource::new(TRANSFER_HEAP_SIZE));
        Self {
            allocator,
            inner: Mutex::new(Inner {
                data: vec![0 as u8; block_count as usize * block_size as usize],
                blocks_written_since_last_barrier: Vec::new(),
            }),
            closed: AtomicBool::new(false),
            operation_closure: Box::new(|_: Op| Ok(())),
            read_only: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            observer: None,
        }
    }

    pub fn set_observer(&mut self, observer: Box<dyn Observer>) {
        self.observer = Some(observer);
    }

    /// Sets a callback that will run at the beginning of read, write, and flush which will forward
    /// any errors, and proceed on Ok().
    pub fn set_op_callback(
        &mut self,
        cb: impl Fn(Op) -> Result<(), Error> + Send + Sync + 'static,
    ) {
        self.operation_closure = Box::new(cb);
    }

    /// Creates a fake block device from an image (which can be anything that implements
    /// std::io::Read).  The size of the device is determined by how much data is read.
    pub fn from_image(
        mut reader: impl std::io::Read,
        block_size: u32,
    ) -> Result<Self, std::io::Error> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        Ok(Self::from_vec(data, block_size))
    }

    /// Creates a fake block device from a `Vec`. The size of the device is determined by the size
    /// of the `Vec`.
    pub fn from_vec(data: Vec<u8>, block_size: u32) -> Self {
        let allocator =
            BufferAllocator::new(block_size as usize, BufferSource::new(TRANSFER_HEAP_SIZE));
        Self {
            allocator,
            inner: Mutex::new(Inner { data, blocks_written_since_last_barrier: Vec::new() }),
            closed: AtomicBool::new(false),
            operation_closure: Box::new(|_| Ok(())),
            read_only: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            observer: None,
        }
    }
}

#[async_trait]
impl Device for FakeDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        assert!(!self.closed.load(Ordering::Relaxed));
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.allocator.block_size() as u32
    }

    fn block_count(&self) -> u64 {
        self.inner.lock().data.len() as u64 / self.block_size() as u64
    }

    async fn read_with_opts(
        &self,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        (self.operation_closure)(Op::Read)?;
        let offset = offset as usize;
        assert_eq!(offset % self.allocator.block_size(), 0);
        let inner = self.inner.lock();
        let size = buffer.len();
        ensure!(
            offset + size <= inner.data.len(),
            "offset: {} len: {} data.len: {}",
            offset,
            size,
            inner.data.len()
        );
        buffer.copy_from_slice(&inner.data[offset..offset + size]);
        Ok(())
    }

    async fn write_with_opts(
        &self,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        ensure!(!self.read_only.load(Ordering::Relaxed));
        let mut inner = self.inner.lock();

        if write_opts.flags.contains(WriteFlags::PRE_BARRIER) {
            if let Some(observer) = &self.observer {
                observer.barrier();
            }
            inner.blocks_written_since_last_barrier.clear();
        }

        (self.operation_closure)(Op::Write)?;
        let offset = offset as usize;
        assert_eq!(offset % self.allocator.block_size(), 0);

        let size = buffer.len();
        ensure!(
            offset + size <= inner.data.len(),
            "offset: {} len: {} data.len: {}",
            offset,
            size,
            inner.data.len()
        );
        buffer.copy_to_slice(&mut inner.data[offset..offset + size]);
        let first_block = offset / self.allocator.block_size();
        for block in first_block..first_block + size / self.allocator.block_size() {
            inner.blocks_written_since_last_barrier.push(block)
        }
        Ok(())
    }

    async fn trim(&self, range: Range<u64>) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        ensure!(!self.read_only.load(Ordering::Relaxed));
        assert_eq!(range.start % self.block_size() as u64, 0);
        assert_eq!(range.end % self.block_size() as u64, 0);
        // Blast over the range to simulate it being used for something else.
        let mut inner = self.inner.lock();
        inner.data[range.start as usize..range.end as usize].fill(0xab);
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        self.closed.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn flush(&self) -> Result<(), Error> {
        self.inner.lock().blocks_written_since_last_barrier.clear();
        (self.operation_closure)(Op::Flush)
    }

    fn reopen(&self, read_only: bool) {
        self.closed.store(false, Ordering::Relaxed);
        self.read_only.store(read_only, Ordering::Relaxed);
    }

    fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    fn supports_trim(&self) -> bool {
        true
    }

    fn snapshot(&self) -> Result<DeviceHolder, Error> {
        let allocator =
            BufferAllocator::new(self.block_size() as usize, BufferSource::new(TRANSFER_HEAP_SIZE));
        Ok(DeviceHolder::new(Self {
            allocator,
            inner: Mutex::new(self.inner.lock().clone()),
            closed: AtomicBool::new(false),
            operation_closure: Box::new(|_: Op| Ok(())),
            read_only: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            observer: None,
        }))
    }

    fn discard_random_since_last_flush(&self) -> Result<(), Error> {
        let bs = self.allocator.block_size();
        use rand::RngExt as _;
        let mut rng = rand::rng();
        let mut guard = self.inner.lock();
        let Inner { data, blocks_written_since_last_barrier, .. } = &mut *guard;
        log::info!("Discarding from {blocks_written_since_last_barrier:?}");
        let mut discarded = Vec::new();
        for block in blocks_written_since_last_barrier.drain(..) {
            if rng.random() {
                data[block * bs..(block + 1) * bs].fill(0xaf);
                discarded.push(block);
            }
        }
        log::info!("Discarded {discarded:?}");
        Ok(())
    }

    /// Sets the poisoned state for the device. A poisoned device will panic the thread that
    /// performs Drop on it.
    fn poison(&self) -> Result<(), Error> {
        self.poisoned.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        if self.poisoned.load(Ordering::Relaxed) {
            panic!("This device was poisoned to crash whomever is holding a reference here.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FakeDevice;
    use crate::Device;
    use block_protocol::{WriteFlags, WriteOptions};

    const TEST_DEVICE_BLOCK_SIZE: usize = 512;

    #[fuchsia::test(threads = 10)]
    async fn test_discard_random_with_barriers() {
        let device = FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE as u32);
        // Loop 100 times to catch errors.
        for _ in 0..1000 {
            let mut data = vec![0; 7 * TEST_DEVICE_BLOCK_SIZE];
            rand::fill(&mut data[..]);
            // Ensure that barriers work with overwrites.
            let indices = [1, 2, 3, 4, 3, 5, 6];
            for i in 0..indices.len() {
                let mut buffer = device.allocate_buffer(TEST_DEVICE_BLOCK_SIZE).await;
                if i == 2 || i == 5 {
                    buffer.copy_from_slice(
                        &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                            ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE],
                    );
                    device
                        .write_with_opts(
                            i as u64 * TEST_DEVICE_BLOCK_SIZE as u64,
                            buffer.as_ref(),
                            WriteOptions { flags: WriteFlags::PRE_BARRIER, ..Default::default() },
                        )
                        .await
                        .expect("Failed to write to FakeDevice");
                } else {
                    buffer.copy_from_slice(
                        &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                            ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE],
                    );
                    device
                        .write_with_opts(
                            i as u64 * TEST_DEVICE_BLOCK_SIZE as u64,
                            buffer.as_ref(),
                            WriteOptions::default(),
                        )
                        .await
                        .expect("Failed to write to FakeDevice");
                }
            }
            device.discard_random_since_last_flush().expect("failed to randomly discard writes");
            let mut discard = false;
            let mut discard_2 = false;
            for i in 0..7 {
                let mut read_buffer = device.allocate_buffer(TEST_DEVICE_BLOCK_SIZE).await;
                device
                    .read(i as u64 * TEST_DEVICE_BLOCK_SIZE as u64, read_buffer.as_mut())
                    .await
                    .expect("failed to read from FakeDevice");
                let mut read_data = vec![0u8; TEST_DEVICE_BLOCK_SIZE];
                read_buffer.copy_to_slice(&mut read_data);
                let expected_data = &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                    ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE];
                if i < 2 {
                    if expected_data != &read_data[..] {
                        discard = true;
                    }
                } else if i < 5 {
                    if discard == true {
                        assert_ne!(expected_data, &read_data[..]);
                        discard_2 = true;
                    } else if expected_data != &read_data[..] {
                        discard_2 = true;
                    }
                } else if discard_2 == true {
                    assert_ne!(expected_data, &read_data[..]);
                }
            }
        }
    }
}
