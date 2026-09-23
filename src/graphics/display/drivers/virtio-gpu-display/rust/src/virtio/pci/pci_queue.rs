// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::pci_notifications::VirtioPciNotificationData;
use crate::virtio::common::queue::VirtioQueue;
use crate::virtio::common::queue::buffer::{
    VirtioBufferRef, VirtioReturnedBufferInfo, VirtioSubmittedBufferId,
};
use crate::virtio::common::queue::layout::VirtioQueuePhysicalMemoryLayout;

use std::num::NonZero;

pub struct VirtioPciQueue {
    common_queue: VirtioQueue,

    /// Used to issue driver notifications identifying the queue.
    notification_data: VirtioPciNotificationData,
}

impl VirtioPciQueue {
    /// Sets up the memory needed to back a virtqueue of a given size.
    ///
    /// `bti` will be used to pin the queue's metadata in physical memory.
    /// `capacity` must be a power of two and must not exceed
    /// [`VirtioQueue::MAX_CAPACITY`].
    pub fn new(
        bti: &zx::Bti,
        capacity: NonZero<u16>,
        notification_data: VirtioPciNotificationData,
    ) -> Result<(Self, VirtioQueuePhysicalMemoryLayout), zx::Status> {
        let (common_queue, memory_layout) = VirtioQueue::new(bti, capacity)?;
        let queue = Self { common_queue, notification_data };

        Ok((queue, memory_layout))
    }

    pub fn notification_data(&self) -> &VirtioPciNotificationData {
        &self.notification_data
    }

    // TODO(https://fxbug.dev/504722357): Add a safe wrapper that manages the
    // queue buffer memory regions.

    /// Submits a buffer containing request/response memory ranges.
    ///
    /// The caller is responsible for notifying the device.
    ///
    /// SAFETY: The [`VirtioMemoryBufferRange`] instances referenced by `buffer`
    /// must remain alive until [`take_returned_buffer()`] returns the same buffer
    /// identifier.
    pub unsafe fn submit_buffer(
        &mut self,
        buffer: VirtioBufferRef<'_>,
    ) -> Result<VirtioSubmittedBufferId, zx::Status> {
        // SAFETY: The called method has the same preconditions as this method.
        unsafe { self.common_queue.submit_buffer(buffer) }
    }

    /// Attempts to extract a buffer from the ring of returned buffers.
    ///
    /// Returns [`None`] if the returned ring is empty. Otherwise, extracts the
    /// first returned buffer, and returns the number of bytes written by the
    /// device.
    pub fn take_returned_buffer(&mut self) -> Option<VirtioReturnedBufferInfo> {
        self.common_queue.take_returned_buffer()
    }
}
