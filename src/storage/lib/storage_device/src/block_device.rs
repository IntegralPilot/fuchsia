// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Device;
use crate::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use crate::buffer_allocator::{BufferAllocator, BufferSource};
use anyhow::{Error, bail, ensure};
use async_trait::async_trait;
use block_client::{
    BlockClient, BlockDeviceFlag, BufferSlice, MutableBufferSlice, ReadOptions, VmoId, WriteOptions,
};
use std::ops::Range;
use zx::Status;

/// BlockDevice is an implementation of Device backed by a real block device behind a FIFO.
pub struct BlockDevice<T> {
    allocator: BufferAllocator,
    remote: T,
    read_only: bool,
    vmoid: VmoId,
}

const TRANSFER_VMO_SIZE: usize = 128 * 1024 * 1024;

impl<T: BlockClient> BlockDevice<T> {
    /// Creates a new BlockDevice over `remote`.
    pub async fn new(remote: T, read_only: bool) -> Result<Self, Error> {
        let buffer_source = BufferSource::new(TRANSFER_VMO_SIZE);
        // SAFETY: We only attach this VMO once here, and we ensure no references are held
        // during I/O via the BufferAllocator and pointer-based BufferRef types.
        let vmoid = unsafe { remote.attach_vmo(buffer_source.vmo()) }.await?;
        let allocator = BufferAllocator::new(remote.block_size() as usize, buffer_source);
        Ok(Self { allocator, remote, read_only, vmoid })
    }

    async fn read_with_opts_internal(
        &self,
        offset: u64,
        buffer: MutableBufferRef<'_>,
        read_opts: ReadOptions,
    ) -> Result<(), Error> {
        if buffer.len() == 0 {
            return Ok(());
        }
        ensure!(self.vmoid.is_valid(), Status::INVALID_ARGS);
        ensure!(offset % (self.block_size() as u64) == 0, Status::INVALID_ARGS);
        ensure!(buffer.range().start % (self.block_size() as usize) == 0, Status::INVALID_ARGS);
        ensure!(buffer.range().end % (self.block_size() as usize) == 0, Status::INVALID_ARGS);
        Ok(self
            .remote
            .read_at_with_opts(
                MutableBufferSlice::new_with_vmo_id(
                    &self.vmoid,
                    buffer.range().start as u64,
                    buffer.len() as u64,
                ),
                offset,
                read_opts,
            )
            .await?)
    }

    async fn write_with_opts_internal(
        &self,
        offset: u64,
        buffer: BufferRef<'_>,
        opts: WriteOptions,
    ) -> Result<(), Error> {
        if self.read_only {
            bail!(Status::ACCESS_DENIED);
        }
        if buffer.len() == 0 {
            return Ok(());
        }
        ensure!(self.vmoid.is_valid(), "Device is closed");
        ensure!(offset % (self.block_size() as u64) == 0, Status::INVALID_ARGS);
        ensure!(buffer.range().start % (self.block_size() as usize) == 0, Status::INVALID_ARGS);
        ensure!(buffer.range().end % (self.block_size() as usize) == 0, Status::INVALID_ARGS);
        Ok(self
            .remote
            .write_at_with_opts(
                BufferSlice::new_with_vmo_id(
                    &self.vmoid,
                    buffer.range().start as u64,
                    buffer.len() as u64,
                ),
                offset,
                opts,
            )
            .await?)
    }
}

#[async_trait]
impl<T: BlockClient> Device for BlockDevice<T> {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn clean_transfer_buffer(&self) {
        self.allocator.clean_transfer_buffer();
    }

    fn block_size(&self) -> u32 {
        self.remote.block_size()
    }

    fn block_count(&self) -> u64 {
        self.remote.block_count()
    }

    async fn read_with_opts(
        &self,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        read_opts: ReadOptions,
    ) -> Result<(), Error> {
        if buffer.allocator_id() != self.allocator.identifier() {
            // Foreign buffer (from another allocator). Force copy!
            let mut temp_buf = self.allocator.allocate_buffer(buffer.len()).await;
            self.read_with_opts_internal(offset, temp_buf.as_mut(), read_opts).await?;
            buffer.as_mut_ptr_slice().copy_from_ptr_slice(temp_buf.as_ptr_slice());
            Ok(())
        } else {
            self.read_with_opts_internal(offset, buffer, read_opts).await
        }
    }

    async fn write_with_opts(
        &self,
        offset: u64,
        buffer: BufferRef<'_>,
        opts: WriteOptions,
    ) -> Result<(), Error> {
        if buffer.allocator_id() != self.allocator.identifier() {
            // Foreign buffer (from another allocator). Force copy!
            let mut temp_buf = self.allocator.allocate_buffer(buffer.len()).await;
            temp_buf.as_mut().as_mut_ptr_slice().copy_from_ptr_slice(buffer.as_ptr_slice());
            self.write_with_opts_internal(offset, temp_buf.as_ref(), opts).await
        } else {
            self.write_with_opts_internal(offset, buffer, opts).await
        }
    }

    async fn trim(&self, range: Range<u64>) -> Result<(), Error> {
        if self.read_only {
            bail!(Status::ACCESS_DENIED);
        }
        ensure!(range.start % (self.block_size() as u64) == 0, Status::INVALID_ARGS);
        ensure!(range.end % (self.block_size() as u64) == 0, Status::INVALID_ARGS);
        Ok(self.remote.trim(range).await?)
    }

    async fn close(&self) -> Result<(), Error> {
        // We can leak the VMO id because we are closing the device.
        let _ = self.vmoid.take().into_id();
        Ok(self.remote.close().await?)
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(self.remote.flush().await?)
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn supports_trim(&self) -> bool {
        self.remote.block_flags().contains(BlockDeviceFlag::TRIM_SUPPORT)
    }
}

impl<T> Drop for BlockDevice<T> {
    fn drop(&mut self) {
        // We can't detach the VmoId because we're not async here, but we are tearing down the
        // connection to the block device so we don't really need to.
        let _ = self.vmoid.take().into_id();
    }
}

#[cfg(test)]
mod tests {
    use crate::Device;
    use crate::block_device::BlockDevice;
    use fake_block_client::FakeBlockClient;
    use zx::Status;

    #[fuchsia::test]
    async fn test_lifecycle() {
        let device =
            BlockDevice::new(FakeBlockClient::new(1024, 1024), false).await.expect("new failed");

        {
            let _buf = device.allocate_buffer(8192).await;
        }

        device.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_write_buffer() {
        let device =
            BlockDevice::new(FakeBlockClient::new(1024, 1024), false).await.expect("new failed");

        {
            let mut buf1 = device.allocate_buffer(8192).await;
            let mut buf2 = device.allocate_buffer(1024).await;
            buf1.fill(0xaa);
            buf2.fill(0xbb);
            device.write(65536, buf1.as_ref()).await.expect("Write failed");
            device.write(65536 + 8192, buf2.as_ref()).await.expect("Write failed");
        }
        {
            let mut buf = device.allocate_buffer(8192 + 1024).await;
            device.read(65536, buf.as_mut()).await.expect("Read failed");
            let mut data = vec![0u8; 8192 + 1024];
            buf.copy_to_slice(&mut data);
            assert_eq!(data[..8192], vec![0xaa as u8; 8192]);
            assert_eq!(data[8192..], vec![0xbb as u8; 1024]);
        }

        device.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_only() {
        let device =
            BlockDevice::new(FakeBlockClient::new(1024, 1024), true).await.expect("new failed");
        let mut buf1 = device.allocate_buffer(8192).await;
        buf1.fill(0xaa);
        let err = device.write(65536, buf1.as_ref()).await.expect_err("Write succeeded");
        assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::ACCESS_DENIED);
    }

    #[fuchsia::test]
    async fn test_unaligned_access() {
        let device =
            BlockDevice::new(FakeBlockClient::new(1024, 1024), false).await.expect("new failed");
        let mut buf1 = device.allocate_buffer(device.block_size() as usize * 2).await;
        buf1.fill(0xaa);

        // Write checks
        {
            let err = device.write(1, buf1.as_ref()).await.expect_err("Write succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
        {
            let err = device
                .write(0, buf1.subslice(1..(device.block_size() as usize + 1)))
                .await
                .expect_err("Write succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
        {
            let err = device
                .write(0, buf1.subslice(1..device.block_size() as usize))
                .await
                .expect_err("Write succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
        {
            let err = device
                .write(0, buf1.subslice(0..(device.block_size() as usize + 1)))
                .await
                .expect_err("Write succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }

        // Read checks
        {
            let err = device.read(1, buf1.as_mut()).await.expect_err("Read succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
        {
            let err = device
                .read(0, buf1.subslice_mut(1..(device.block_size() as usize + 1)))
                .await
                .expect_err("Read succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
        {
            let err = device
                .read(0, buf1.subslice_mut(1..device.block_size() as usize))
                .await
                .expect_err("Read succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
        {
            let err = device
                .read(0, buf1.subslice_mut(0..(device.block_size() as usize + 1)))
                .await
                .expect_err("Read succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }

        // Trim
        {
            let err = device.trim(1..device.block_size() as u64).await.expect_err("Read succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
        {
            let err =
                device.trim(1..(device.block_size() as u64 + 1)).await.expect_err("Read succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
        {
            let err =
                device.trim(0..(device.block_size() as u64 + 1)).await.expect_err("Read succeeded");
            assert_eq!(err.root_cause().downcast_ref::<Status>().unwrap(), &Status::INVALID_ARGS);
        }
    }

    #[fuchsia::test]
    async fn test_foreign_buffer_read_write() {
        let device1 =
            BlockDevice::new(FakeBlockClient::new(1024, 1024), false).await.expect("new failed");
        let device2 =
            BlockDevice::new(FakeBlockClient::new(1024, 1024), false).await.expect("new failed");

        // Write data using a buffer allocated from device2 to device1 (foreign write)
        let mut foreign_buf = device2.allocate_buffer(8192).await;
        foreign_buf.fill(0xaa);
        device1.write(0, foreign_buf.as_ref()).await.expect("Foreign write failed");

        // Read data back using a buffer allocated from device2 from device1 (foreign read)
        let mut foreign_read_buf = device2.allocate_buffer(8192).await;
        device1.read(0, foreign_read_buf.as_mut()).await.expect("Foreign read failed");

        let mut data = vec![0u8; 8192];
        foreign_read_buf.copy_to_slice(&mut data);
        assert_eq!(data, vec![0xaa; 8192]);
    }
}
