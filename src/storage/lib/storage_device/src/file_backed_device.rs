// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    crate::{
        Device,
        buffer::{BufferFuture, BufferRef, MutableBufferRef},
        buffer_allocator::{BufferAllocator, BufferSource},
    },
    anyhow::{Error, ensure},
    async_trait::async_trait,
    block_protocol::{ReadOptions, WriteOptions},
    // Provides read_exact_at and write_all_at.
    std::{ops::Range, os::unix::fs::FileExt},
};

/// FileBackedDevice is an implementation of Device backed by a std::fs::File. It is intended to be
/// used for host tooling (to create or verify fxfs images), although it could also be used on
/// Fuchsia builds if we wanted to do that for whatever reason.
pub struct FileBackedDevice {
    allocator: BufferAllocator,
    file: std::fs::File,
    block_count: u64,
    block_size: u32,
}

const TRANSFER_HEAP_SIZE: usize = 32 * 1024 * 1024;

impl FileBackedDevice {
    /// Creates a new FileBackedDevice over `file`. The size of the file will be used as the size of
    /// the Device.
    pub fn new(file: std::fs::File, block_size: u32) -> Self {
        let size = file.metadata().unwrap().len();
        assert!(block_size > 0 && size > 0);
        Self::new_with_block_count(file, block_size, size / block_size as u64)
    }

    /// Creates a new FileBackedDevice over `file` using an explicit size.  The underlying file is
    /// *not* truncated to the target size, so the file size will be exactly as large as the
    /// filesystem ends up using within the file.  With a sequential allocator, this makes the file
    /// as big as it needs to be and no more.
    pub fn new_with_block_count(file: std::fs::File, block_size: u32, block_count: u64) -> Self {
        // NOTE: If file is S_ISBLK, we could (and probably should) use its block size. Rust does
        // not appear to expose this information in a portable way, so we may need to dip into
        // non-portable code to do so.
        let allocator =
            BufferAllocator::new(block_size as usize, BufferSource::new(TRANSFER_HEAP_SIZE));
        Self { allocator, file, block_count, block_size }
    }
}

#[async_trait]
impl Device for FileBackedDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }

    async fn read_with_opts(
        &self,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        assert_eq!(offset % self.block_size() as u64, 0);
        assert_eq!(buffer.range().start % self.block_size() as usize, 0);
        assert_eq!(buffer.len() % self.block_size() as usize, 0);
        ensure!(offset + buffer.len() as u64 <= self.size(), "Reading past end of file");
        // This isn't actually async, but that probably doesn't matter for host usage.
        if let Some(slice) = buffer.try_as_mut_slice() {
            self.file.read_exact_at(slice, offset)?;
        } else {
            let mut data = vec![0u8; buffer.len()];
            self.file.read_exact_at(&mut data, offset)?;
            buffer.copy_from_slice(&data);
        }
        Ok(())
    }

    async fn write_with_opts(
        &self,
        offset: u64,
        buffer: BufferRef<'_>,
        _write_opts: WriteOptions,
    ) -> Result<(), Error> {
        assert_eq!(offset % self.block_size() as u64, 0);
        assert_eq!(buffer.range().start % self.block_size() as usize, 0);
        assert_eq!(buffer.len() % self.block_size() as usize, 0);
        ensure!(offset + buffer.len() as u64 <= self.size(), "Writing past end of file");
        // This isn't actually async, but that probably doesn't matter for host usage.
        if let Some(slice) = buffer.try_as_slice() {
            self.file.write_all_at(slice, offset)?;
        } else {
            let data = buffer.to_vec();
            self.file.write_all_at(&data, offset)?;
        }
        Ok(())
    }

    async fn trim(&self, range: Range<u64>) -> Result<(), Error> {
        assert_eq!(range.start % self.block_size() as u64, 0);
        assert_eq!(range.end % self.block_size() as u64, 0);
        // Blast over the range to simulate it being used for something else.
        // This will help catch incorrect usage of trim, and since FileBackedDevice is not used in a
        // production context, there should be no performance issues.
        // Note that we could punch a hole in the file instead using platform-dependent operations
        // (e.g. FALLOC_FL_PUNCH_HOLE on Linux) to speed this up if needed.
        const BUF: [u8; 8192] = [0xab; 8192];
        let mut offset = range.start;
        while offset < range.end {
            let len = std::cmp::min(BUF.len(), range.end as usize - offset as usize);
            self.file.write_at(&BUF[..len], offset)?;
            offset += len as u64;
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        // This isn't actually async, but that probably doesn't matter for host usage.
        self.file.sync_all()?;
        Ok(())
    }

    async fn flush(&self) -> Result<(), Error> {
        self.file.sync_data().map_err(Into::into)
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn supports_trim(&self) -> bool {
        // We "support" trim insofar as Device::trim() can be called.  The actual implementation is,
        // of course, simulated.
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::Device;
    use crate::file_backed_device::FileBackedDevice;
    use std::fs::{File, OpenOptions};
    use std::path::PathBuf;

    fn create_file() -> (PathBuf, File) {
        let mut temp_path = std::env::temp_dir();
        temp_path.push(format!("file_{:x}", rand::random::<u64>()));
        let (pathbuf, file) = (
            temp_path.clone(),
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(temp_path.as_path())
                .unwrap_or_else(|e| panic!("create {:?} failed: {:?}", temp_path.as_path(), e)),
        );
        file.set_len(1024 * 1024).expect("Failed to truncate file");
        (pathbuf, file)
    }

    #[fuchsia::test]
    async fn test_lifecycle() {
        let (_path, file) = create_file();
        let device = FileBackedDevice::new(file, 512);

        {
            let _buf = device.allocate_buffer(8192).await;
        }

        device.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_write() {
        let (_path, file) = create_file();
        let device = FileBackedDevice::new(file, 512);

        {
            let mut buf1 = device.allocate_buffer(8192).await;
            let mut buf2 = device.allocate_buffer(8192).await;
            buf1.fill(0xaa);
            buf2.fill(0xbb);
            device.write(65536, buf1.as_ref()).await.expect("Write failed");
            device.write(65536 + 8192, buf2.as_ref()).await.expect("Write failed");
        }
        {
            let mut buf = device.allocate_buffer(16384).await;
            device.read(65536, buf.as_mut()).await.expect("Read failed");
            let mut data = vec![0u8; 16384];
            buf.copy_to_slice(&mut data);
            assert_eq!(data[..8192], vec![0xaa as u8; 8192]);
            assert_eq!(data[8192..], vec![0xbb as u8; 8192]);
        }

        device.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_write_past_end_of_file_fails() {
        let (_path, file) = create_file();
        let device = FileBackedDevice::new(file, 512);

        {
            let mut buf = device.allocate_buffer(8192).await;
            let offset = (device.size() as usize - buf.len() + device.block_size() as usize) as u64;
            buf.fill(0xaa);
            device.write(offset, buf.as_ref()).await.expect_err("Write should have failed");
            device.read(offset, buf.as_mut()).await.expect_err("Read should have failed");
        }

        device.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_writes_persist() {
        let (path, file) = create_file();
        let device = FileBackedDevice::new(file, 512);

        {
            let mut buf1 = device.allocate_buffer(8192).await;
            let mut buf2 = device.allocate_buffer(8192).await;
            buf1.fill(0xaa);
            buf2.fill(0xbb);
            device.write(65536, buf1.as_ref()).await.expect("Write failed");
            device.write(65536 + 8192, buf2.as_ref()).await.expect("Write failed");
        }
        device.close().await.expect("Close failed");

        let file = File::open(path.as_path()).expect("Open failed");
        let device = FileBackedDevice::new(file, 512);

        {
            let mut buf = device.allocate_buffer(16384).await;
            device.read(65536, buf.as_mut()).await.expect("Read failed");
            let mut data = vec![0u8; 16384];
            buf.copy_to_slice(&mut data);
            assert_eq!(data[..8192], vec![0xaa as u8; 8192]);
            assert_eq!(data[8192..], vec![0xbb as u8; 8192]);
        }
        device.close().await.expect("Close failed");
    }
}
