// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of a virtual storage devices backed by a [`ReadObjectHandle`]. Allows using
//! files within an existing fxfs as a virtual storage device (e.g. to mount an inner filesystem).

use crate::errors::FxfsError;
use crate::object_handle::ReadObjectHandle;
use anyhow::{Context as _, Error, bail};
use async_trait::async_trait;
use std::ops::Range;
use storage_device::buffer::MutableBufferRef;
use storage_device::buffer_allocator::BufferFuture;
use storage_device::{Device, ReadOptions};

/// Allows using anything that implements [`ReadObjectHandle`] as a read-only storage [`Device`].
pub struct ReadOnlyDevice<H: ReadObjectHandle> {
    handle: H,
}

impl<H: ReadObjectHandle> ReadOnlyDevice<H> {
    pub fn new(handle: H) -> Result<Self, Error> {
        let device = Self { handle };
        // Prevent division by zero when calculating the block count.
        if device.block_size() == 0 {
            bail!("Expected non-zero block size.");
        }
        Ok(device)
    }
}

#[async_trait]
impl<H: ReadObjectHandle> Device for ReadOnlyDevice<H> {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.handle.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.handle.block_size().get() as u32
    }

    fn block_count(&self) -> u64 {
        self.handle.get_size() / self.handle.block_size()
    }

    async fn read_with_opts(
        &self,
        offset: u64,
        buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        let len = buffer.len();
        let amount = self.handle.read(offset, buffer).await?;
        if amount != len {
            return Err(FxfsError::OutOfRange).context("short read from underlying object");
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn supports_trim(&self) -> bool {
        false
    }

    async fn write_with_opts(
        &self,
        _offset: u64,
        _buffer: storage_device::buffer::BufferRef<'_>,
        _write_opts: storage_device::WriteOptions,
    ) -> Result<(), Error> {
        unreachable!()
    }

    async fn flush(&self) -> Result<(), Error> {
        unreachable!()
    }

    async fn trim(&self, _range: Range<u64>) -> Result<(), Error> {
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::FxFilesystem;
    use crate::object_handle::ObjectHandle as _;
    use crate::object_store::transaction::{LockKey, lock_keys};
    use crate::object_store::volume::root_volume;
    use crate::object_store::{DataObjectHandle, Directory, NewChildStoreOptions, ObjectStore};
    use std::sync::Arc;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    /// Helper function that creates a test file filled with a known byte pattern.
    /// Each block in the file will be filled with the block offset mod 0xFF.
    async fn create_test_file(
        fs: &Arc<FxFilesystem>,
        num_blocks: usize,
    ) -> DataObjectHandle<ObjectStore> {
        let root_vol = root_volume(fs.clone()).await.unwrap();
        let store = root_vol.new_volume("test", NewChildStoreOptions::default()).await.unwrap();
        let test_vol_root =
            Directory::open(&store, store.root_directory_object_id()).await.unwrap();

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Default::default(),
            )
            .await
            .unwrap();

        let object = test_vol_root.create_child_file(&mut transaction, "test_file").await.unwrap();
        transaction.commit().await.unwrap();

        {
            let mut transaction = object.new_transaction().await.unwrap();
            let block_size = object.block_size().get() as usize;
            let mut buffer = object.allocate_buffer(block_size * num_blocks).await;
            for i in 0..num_blocks {
                let buff_range = (i * block_size)..((i + 1) * block_size);
                buffer.subslice_mut(buff_range).fill((i % 0xFF) as u8);
            }
            object.txn_write(&mut transaction, 0, buffer.as_ref()).await.unwrap();

            transaction.commit().await.unwrap();
        }

        object
    }

    #[fuchsia::test]
    async fn test_read_only_virtual_device() {
        const BLOCK_SIZE: usize = 4096;
        const TEST_FILE_BLOCK_COUNT: usize = 64;
        let device = DeviceHolder::new(FakeDevice::new(512, BLOCK_SIZE as u32));
        let fs = FxFilesystem::new_empty(device).await.unwrap();
        let handle = create_test_file(&fs, TEST_FILE_BLOCK_COUNT).await;
        let handle_as_device = ReadOnlyDevice::new(handle).unwrap();
        assert_eq!(handle_as_device.block_size(), BLOCK_SIZE as u32);
        assert_eq!(handle_as_device.block_count(), TEST_FILE_BLOCK_COUNT as u64);

        // We should be able to read the whole file through our virtual read-only device.
        let mut buffer = handle_as_device.allocate_buffer(BLOCK_SIZE * TEST_FILE_BLOCK_COUNT).await;
        handle_as_device.read(0, buffer.as_mut()).await.unwrap();
        let data = buffer.to_vec();
        for i in 0..TEST_FILE_BLOCK_COUNT {
            let buff_range = (i * BLOCK_SIZE)..((i + 1) * BLOCK_SIZE);
            assert_eq!(data[buff_range], [(i % 0xFF) as u8; BLOCK_SIZE]);
        }

        // Test reading from an offset.
        let mut buffer = handle_as_device.allocate_buffer(BLOCK_SIZE).await;
        handle_as_device.read((BLOCK_SIZE * 4) as u64, buffer.as_mut()).await.unwrap();
        assert_eq!(buffer.to_vec(), [4u8; BLOCK_SIZE]);
    }
}
