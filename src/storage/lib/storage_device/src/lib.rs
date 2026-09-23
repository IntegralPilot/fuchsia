// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `storage_device` provides a higher-level portable API ([`Device`]) for interacting with block
//! devices.  This library also provides the [`Buffer`] type which is a contiguous, splittable
//! transfer buffer allocated out of a shared pool which can be used for I/O.
//!
//! The two main implementations are
//!   - [`block_device::BlockDevice`], which is backed by a [`block_client::Device`] and used on
//!     Fuchsia devices, and
//!   - [`file_backed_device::FileBackedDevice`], which is backed by a regular file and is portable.

use crate::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use anyhow::{Error, bail};
use async_trait::async_trait;
// pub so `Device` trait implementations don't need to depend on the `block_protocol` crate
pub use block_protocol::{InlineCryptoOptions, ReadOptions, WriteFlags, WriteOptions};
use futures::channel::oneshot::{Sender, channel};
use std::future::Future;
use std::mem::ManuallyDrop;
use std::ops::{Deref, Range};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

pub mod buffer;
pub mod buffer_allocator;
pub mod splittable_buffer;

pub use splittable_buffer::{SplittableBuffer, SubHandle};

#[cfg(target_os = "fuchsia")]
pub mod block_device;
#[cfg(target_os = "fuchsia")]
pub mod pinned_buffer_allocator;

#[cfg(target_family = "unix")]
pub mod file_backed_device;

pub mod fake_device;

pub mod ranged_device;

#[async_trait]
/// Device is an abstract representation of an underlying block device.
pub trait Device: Send + Sync {
    /// Allocates a transfer buffer of at least `size` bytes for doing I/O with the device.
    /// The actual size of the buffer will be rounded up to a block-aligned size.
    /// Blocks until enough capacity is available in the buffer.
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_>;

    /// Cleans up any transfer buffers that are no longer in use.
    fn clean_transfer_buffer(&self) {}

    /// Returns the block size of the device. Buffers are aligned to block-aligned chunks.
    fn block_size(&self) -> u32;

    /// Returns the number of blocks of the device.
    fn block_count(&self) -> u64;

    /// Returns the size in bytes of the device.
    fn size(&self) -> u64 {
        self.block_size() as u64 * self.block_count()
    }

    /// Fills `buffer` with blocks read from `offset`.
    fn read<'a>(
        &'a self,
        offset: u64,
        buffer: MutableBufferRef<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        self.read_with_opts(offset, buffer, ReadOptions::default())
    }

    /// Fills `buffer` with blocks read from `offset`.
    async fn read_with_opts(
        &self,
        offset: u64,
        buffer: MutableBufferRef<'_>,
        read_opts: ReadOptions,
    ) -> Result<(), Error>;

    /// Writes the contents of `buffer` to the device at `offset`.
    fn write<'a>(
        &'a self,
        offset: u64,
        buffer: BufferRef<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        self.write_with_opts(offset, buffer, WriteOptions::default())
    }

    /// Writes the contents of `buffer` to the device at `offset`.
    async fn write_with_opts(
        &self,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error>;

    /// Trims the given device `range`.
    async fn trim(&self, range: Range<u64>) -> Result<(), Error>;

    /// Closes the block device. It is an error to continue using the device after this, but close
    /// itself is idempotent.
    async fn close(&self) -> Result<(), Error>;

    /// Flush the device.
    async fn flush(&self) -> Result<(), Error>;

    /// Reopens the device, making it usable again. (Only implemented for testing devices.)
    fn reopen(&self, _read_only: bool) {
        unreachable!();
    }
    /// Returns whether the device is read-only.
    fn is_read_only(&self) -> bool;

    /// Returns whether the device supports trim.
    fn supports_trim(&self) -> bool;

    /// Returns a snapshot of the device.
    fn snapshot(&self) -> Result<DeviceHolder, Error> {
        bail!("Not supported");
    }

    /// Discards random blocks since the last flush.
    fn discard_random_since_last_flush(&self) -> Result<(), Error> {
        bail!("Not supported");
    }

    /// Poisons a device to panic on drop. Used to find hanging references.
    fn poison(&self) -> Result<(), Error> {
        bail!("Not supported");
    }
}

// Arc<dyn Device> can easily be cloned and supports concurrent access, but sometimes exclusive
// access is required, in which case APIs should accept DeviceHolder.  It doesn't guarantee there
// aren't some users that hold an Arc<dyn Device> somewhere, but it does mean that something that
// accepts a DeviceHolder won't be sharing the device with something else that accepts a
// DeviceHolder.  For example, FxFilesystem accepts a DeviceHolder which means that you cannot
// create two FxFilesystem instances that are both sharing the same device.
pub struct DeviceHolder {
    device: ManuallyDrop<Arc<dyn Device>>,
    on_drop: OnceLock<Sender<DeviceHolder>>,
}

impl DeviceHolder {
    pub fn new(device: impl Device + 'static) -> Self {
        DeviceHolder { device: ManuallyDrop::new(Arc::new(device)), on_drop: OnceLock::new() }
    }

    // Ensures there are no dangling references to the device. Useful for tests to ensure orderly
    // shutdown.
    pub fn ensure_unique(&self) {
        assert_eq!(Arc::strong_count(&self.device), 1);
    }

    pub fn take_when_dropped(&self) -> impl Future<Output = DeviceHolder> + use<> {
        let (sender, receiver) = channel::<DeviceHolder>();
        self.on_drop
            .set(sender)
            .unwrap_or_else(|_| panic!("take_when_dropped should only be called once"));
        async { receiver.await.unwrap() }
    }
}

impl Drop for DeviceHolder {
    fn drop(&mut self) {
        if let Some(sender) = self.on_drop.take() {
            // SAFETY: `device` is not used again.
            let device = ManuallyDrop::new(unsafe { ManuallyDrop::take(&mut self.device) });
            // We don't care if this fails to send.
            let _ = sender.send(DeviceHolder { device, on_drop: OnceLock::new() });
        } else {
            // SAFETY: `device` is not used again.
            unsafe { ManuallyDrop::drop(&mut self.device) }
        }
    }
}

impl Deref for DeviceHolder {
    type Target = Arc<dyn Device>;

    fn deref(&self) -> &Self::Target {
        &self.device
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceHolder;
    use crate::fake_device::FakeDevice;

    #[fuchsia::test]
    async fn test_take_when_dropped() {
        let holder = DeviceHolder::new(FakeDevice::new(1, 512));
        let fut = holder.take_when_dropped();
        std::mem::drop(holder);
        fut.await.ensure_unique();
    }
}
