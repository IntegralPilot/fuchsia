// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Split virtqueue algorithms.

pub mod abi;
pub mod buffer;
pub mod descriptor_table;
pub mod layout;
pub mod rings;

use crate::virtio::common::queue::buffer::{
    VirtioBufferRef, VirtioReturnedBufferInfo, VirtioSubmittedBufferId,
};
use crate::virtio::common::queue::descriptor_table::VirtioQueueDescriptorTable;
use crate::virtio::common::queue::layout::{
    VirtioQueueMemoryLayout, VirtioQueuePartsBuilder, VirtioQueuePhysicalMemoryLayout,
};
use crate::virtio::common::queue::rings::{VirtioQueueReturnedRing, VirtioQueueSubmittedRing};

use std::num::NonZero;
use zx::Vmo;
use zx_sys::zx_paddr_t;

/// Represents a single split virtqueue.
// @cite(virtio): sec="2.6" title="Virtqueues"
// @cite(virtio): sec="2.7" title="Split Virtqueues"
pub struct VirtioQueue {
    /// The contiguous memory region backing the virtqueue's metadata.
    #[expect(dead_code)]
    vmo: Vmo,

    /// Keeps the virtqueue's metadata pinned in physical memory.
    ///
    /// The virtqueue's physical memory will be quarantined when the queue is
    /// dropped. This is appropriate, because the virtio device may continue to access
    /// the physical memory until it is explicitly reset.
    ///
    /// If we implement the support for stopping a virtio device, the driver's
    /// stop method should call a method that explicitly consumes the queue and
    /// releases its backing memory.
    #[expect(dead_code)]
    pmt: zx::Pmt,

    /// Manages the virtqueue's descriptor table.
    descriptor_table: VirtioQueueDescriptorTable,

    /// Manages the virtqueue's ring of submitted buffers.
    submitted_ring: VirtioQueueSubmittedRing,

    /// Manages the virtqueue's ring of returned buffers.
    returned_ring: VirtioQueueReturnedRing,
}

impl VirtioQueue {
    #[expect(dead_code)]
    pub const MAX_CAPACITY: u16 = VirtioQueueMemoryLayout::MAX_CAPACITY;

    /// Sets up the memory needed to back a virtqueue of a given size.
    ///
    /// `bti` will be used to pin the queue's metadata in physical memory.
    /// `capacity` must be a power of two and must not exceed [`MAX_CAPACITY`].
    pub fn new(
        bti: &zx::Bti,
        capacity: NonZero<u16>,
    ) -> Result<(Self, VirtioQueuePhysicalMemoryLayout), zx::Status> {
        debug_assert!(!bti.is_invalid());

        let page_size = zx::system_get_page_size() as usize;
        debug_assert!(page_size > 0, "zx::system_get_page_size() returned 0");
        debug_assert!(
            page_size.is_power_of_two(),
            "zx::system_get_page_size() returned non-power-of-two: {}",
            page_size
        );
        let page_size = NonZero::<usize>::new(page_size).unwrap();

        let queue_layout = VirtioQueueMemoryLayout::new(capacity, page_size);

        // Zircon returns zeroed memory. We don't need to initialize it ourselves.
        let queue_data_vmo = Vmo::create_contiguous(bti, queue_layout.total_size().get(), 0)
            .map_err(|_| zx::Status::NO_MEMORY)?;

        // Pinning one contiguous VMO returns a single address.
        let mut pinned_physical_addresses: [zx_paddr_t; 1] = [0];
        let queue_data_pmt = bti
            .pin(
                zx::BtiOptions::PERM_READ | zx::BtiOptions::PERM_WRITE | zx::BtiOptions::CONTIGUOUS,
                &queue_data_vmo,
                /*offset=*/ 0,
                queue_layout.total_size().get() as u64,
                &mut pinned_physical_addresses,
            )
            .map_err(|_| zx::Status::INTERNAL)?;

        let queue_data_physical_address = pinned_physical_addresses[0] as u64;
        let physical_memory_layout =
            VirtioQueuePhysicalMemoryLayout::new(&queue_layout, queue_data_physical_address);

        let mut parts_builder = VirtioQueuePartsBuilder::new(&queue_layout, &queue_data_vmo)?;

        let queue = Self {
            vmo: queue_data_vmo,
            pmt: queue_data_pmt,
            descriptor_table: parts_builder.build_descriptor_table(),
            submitted_ring: parts_builder.build_submitted_ring(),
            returned_ring: parts_builder.build_returned_ring(),
        };

        Ok((queue, physical_memory_layout))
    }

    // TODO(https://fxbug.dev/504722357): Add a safe wrapper that manages the
    // queue buffer memory regions.

    /// Submits a buffer containing request/response memory ranges.
    ///
    /// The caller is responsible for issuing a memory barrier and notifying the
    /// device that the virtqueue was modified. The specification encourages
    /// using a single notification for multiple modifications (batching) when
    /// possible.
    ///
    /// The caller is responsible for ensuring that the queue's descriptor table
    /// has at least [`VirtioBufferRef::len`] free entries. This precondition
    /// can be easily turned into an error if necessary.
    ///
    /// SAFETY: The [`VirtioMemoryBufferRange`] instances referenced by `buffer`
    /// must remain alive until a call to [`take_returned_buffer()`] returns the
    /// same [`VirtioSubmittedBufferId`].
    // @cite(virtio): sec="2.7.13" title="Supplying Buffers to The Device"
    // @cite(virtio): sec="2.8.21.1" title="Placing Available Buffers Into The Descriptor Ring"
    // @alias(virtio): theirs="batching"
    pub unsafe fn submit_buffer(
        &mut self,
        buffer: VirtioBufferRef<'_>,
    ) -> Result<VirtioSubmittedBufferId, zx::Status> {
        // @cite(virtio): sec="2.7.13" title="Supplying Buffers to The Device"

        let descriptor_list_head = self
            .descriptor_table
            .take_descriptors(buffer.len())
            .expect("Virtqueue descriptor table exhausted by pending requests");

        let submitted_buffer_id: VirtioSubmittedBufferId = (&descriptor_list_head).into();

        let mut current_index_option = Some(descriptor_list_head.first_index());

        for range in buffer.as_ref() {
            let current_index =
                current_index_option.expect("Allocated chain is shorter than expected");
            self.descriptor_table.set_descriptor(current_index, range);
            current_index_option = self.descriptor_table.read_descriptor_next_index(current_index);
        }

        self.submitted_ring.push_back(descriptor_list_head);

        Ok(submitted_buffer_id)
    }

    /// Attempts to extract a buffer from the ring of returned buffers.
    ///
    /// Returns [`None`] if the returned ring is empty. Otherwise, extracts the
    /// first returned buffer, and returns the number of bytes written by the
    /// device.
    pub fn take_returned_buffer(&mut self) -> Option<VirtioReturnedBufferInfo> {
        let returned_buffer = self.returned_ring.pop_front()?;

        let returned_buffer_info: VirtioReturnedBufferInfo = (&returned_buffer).into();

        self.descriptor_table.return_descriptors(returned_buffer.list_head);
        Some(returned_buffer_info)
    }
}
