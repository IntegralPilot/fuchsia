// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::log::*;
use crate::object_handle::{ObjectHandle, ReadObjectHandle};
use crate::object_store::journal::JournalHandle;
use crate::range::RangeExt;
use anyhow::Error;
use async_trait::async_trait;
use std::cmp::min;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use storage_device::Device;
use storage_device::buffer::{BufferFuture, MutableBufferRef};
use storage_units::BlockSize;

// Extents are logically contiguous, so we don't need to store their start offset.
#[derive(Debug, Clone)]
struct Extent {
    // Keep track of the offset of the transaction in which the extent was added, which is necessary
    // for discard_extents.
    added_offset: u64,
    device_range: Range<u64>,
}
/// To read the super-block and journal, we use this handle since we cannot use DataObjectHandle
/// until we've replayed the whole journal.  Clients must supply the extents to be used.
pub struct BootstrapObjectHandle {
    object_id: u64,
    device: Arc<dyn Device>,
    start_offset: u64,
    end_offset: u64,

    /// A list of extents we know of for the handle; they are logically contiguous from
    /// `start_offset`.
    extents: Vec<Extent>,

    /// An extent that is used to bootstrap reading this handle but will be replaced
    /// on the first call to 'push_extent'. This lets us bootstrap reading of a superblock that
    /// self-describes its own extents.
    initial_extent: Option<Extent>,
    trace: AtomicBool,
}

impl BootstrapObjectHandle {
    pub fn new(object_id: u64, device: Arc<dyn Device>, initial_extent: Range<u64>) -> Self {
        Self {
            object_id,
            device,
            start_offset: 0,
            end_offset: initial_extent.end - initial_extent.start,
            extents: Vec::new(),
            initial_extent: Some(Extent { added_offset: 0, device_range: initial_extent }),
            trace: AtomicBool::new(false),
        }
    }

    pub fn new_with_start_offset(
        object_id: u64,
        device: Arc<dyn Device>,
        start_offset: u64,
    ) -> Self {
        Self {
            object_id,
            device,
            start_offset,
            end_offset: start_offset,
            extents: Vec::new(),
            initial_extent: None,
            trace: AtomicBool::new(false),
        }
    }
}

impl ObjectHandle for BootstrapObjectHandle {
    fn object_id(&self) -> u64 {
        self.object_id
    }

    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.device.allocate_buffer(size)
    }

    fn block_size(&self) -> BlockSize {
        BlockSize::new(self.device.block_size()).unwrap()
    }

    fn set_trace(&self, trace: bool) {
        let old_value = self.trace.swap(trace, Ordering::Relaxed);
        if trace != old_value {
            info!(oid = self.object_id, trace; "JH: trace");
        }
    }
}

#[async_trait]
impl ReadObjectHandle for BootstrapObjectHandle {
    async fn read(&self, mut offset: u64, mut buf: MutableBufferRef<'_>) -> Result<usize, Error> {
        assert!(offset >= self.start_offset);
        let trace = self.trace.load(Ordering::Relaxed);
        if trace {
            info!(len = buf.len(), offset; "JH: read");
        }
        let len = buf.len();
        let mut buf_offset = 0;
        let mut file_offset = self.start_offset;
        let extents = if let Some(initial_extent) = &self.initial_extent {
            std::slice::from_ref(initial_extent)
        } else {
            &self.extents
        };
        for extent in extents {
            let device_range = &extent.device_range;
            let extent_len = device_range.end - device_range.start;
            if offset < file_offset + extent_len {
                if trace {
                    info!(device_range:?; "JH: matching extent");
                }
                let device_offset = device_range.start + offset - file_offset;
                let to_read =
                    min(device_range.end - device_offset, (len - buf_offset) as u64) as usize;
                assert!(buf_offset % self.device.block_size() as usize == 0);
                self.device
                    .read(
                        device_offset,
                        buf.reborrow().subslice_mut(buf_offset..buf_offset + to_read),
                    )
                    .await?;
                buf_offset += to_read;
                if buf_offset == len {
                    break;
                }
                offset += to_read as u64;
            }
            file_offset += extent_len;
        }
        Ok(buf_offset)
    }

    fn get_size(&self) -> u64 {
        self.end_offset
    }
}

impl JournalHandle for BootstrapObjectHandle {
    fn end_offset(&self) -> Option<u64> {
        Some(self.end_offset)
    }

    fn push_extent(&mut self, added_offset: u64, device_range: Range<u64>) {
        if self.initial_extent.is_some() {
            self.initial_extent = None;
            self.end_offset = 0;
        }
        self.end_offset += device_range.length().unwrap();
        debug_assert!(
            self.extents.last().map_or(true, |e| e.added_offset <= added_offset),
            "last extent added at {}; this added at {added_offset}",
            self.extents.last().unwrap().added_offset
        );
        self.extents.push(Extent { added_offset, device_range });
    }

    fn discard_extents(&mut self, discard_offset: u64) {
        let index = self.extents.partition_point(|extent| extent.added_offset < discard_offset);
        if index == self.extents.len() {
            return;
        }
        let discarded = self.extents.drain(index..);
        let trace = self.trace.load(Ordering::Relaxed);
        for extent in discarded {
            self.end_offset -= extent.device_range.length().unwrap();
            if trace {
                info!(discard_offset, extent:?; "JH: Discarded extent");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BootstrapObjectHandle;
    use crate::object_handle::ReadObjectHandle as _;
    use crate::object_store::journal::JournalHandle as _;
    use std::sync::Arc;
    use storage_device::Device as _;
    use storage_device::fake_device::FakeDevice;

    #[fuchsia::test]
    async fn test_initial_extent() {
        let device = Arc::new(FakeDevice::new(64, 512));
        let mut buffer = device.allocate_buffer(1024).await;
        buffer.fill(1);
        device.write(0, buffer.as_ref()).await.unwrap();
        buffer.fill(2);
        device.write(1024, buffer.as_ref()).await.unwrap();
        buffer.fill(0);

        let handle = BootstrapObjectHandle::new(1, device.clone(), 0..0);
        assert_eq!(handle.get_size(), 0);
        assert_eq!(handle.read(0, buffer.as_mut()).await.expect("no initial data"), 0);

        let mut handle = BootstrapObjectHandle::new(1, device.clone(), 1024..2048);
        assert_eq!(handle.get_size(), 1024);
        handle.read(0, buffer.as_mut()).await.expect("read implicit extent");
        assert_eq!(buffer.to_vec(), vec![2u8; 1024]);
        handle.push_extent(0, 0..1024);
        handle.read(0, buffer.as_mut()).await.expect("read first explicit extent");
        assert_eq!(buffer.to_vec(), vec![1u8; 1024]);
    }

    #[fuchsia::test]
    async fn test_discard_extents() {
        let device = Arc::new(FakeDevice::new(64, 512));
        let mut handle = BootstrapObjectHandle::new(1, device.clone(), 0..0);

        let mut buffer = device.allocate_buffer(1024).await;
        buffer.fill(1);
        device.write(0, buffer.as_ref()).await.unwrap();
        buffer.fill(2);
        device.write(1024, buffer.as_ref()).await.unwrap();
        buffer.fill(0);

        handle.push_extent(0, 1024..2048);
        handle.push_extent(131072, 0..1024);

        assert_eq!(handle.get_size(), 2048);
        handle.read(0, buffer.as_mut()).await.unwrap();
        assert_eq!(buffer.to_vec(), vec![2u8; 1024]);
        handle.read(1024, buffer.as_mut()).await.unwrap();
        assert_eq!(buffer.to_vec(), vec![1u8; 1024]);

        // Discard at an offset greater than any extent was added, which should be a NOP.
        handle.discard_extents(131073);

        assert_eq!(handle.get_size(), 2048);
        assert_eq!(handle.read(0, buffer.as_mut()).await.unwrap(), 1024);
        assert_eq!(buffer.to_vec(), vec![2u8; 1024]);
        assert_eq!(handle.read(1024, buffer.as_mut()).await.unwrap(), 1024);
        assert_eq!(buffer.to_vec(), vec![1u8; 1024]);

        // Discard the second extent.
        handle.discard_extents(131072);

        assert_eq!(handle.get_size(), 1024);
        assert_eq!(handle.read(0, buffer.as_mut()).await.unwrap(), 1024);
        assert_eq!(buffer.to_vec(), vec![2u8; 1024]);
        assert_eq!(handle.read(1024, buffer.as_mut()).await.unwrap(), 0);
    }
}
