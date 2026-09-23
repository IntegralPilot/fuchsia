// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Straightforward memory layout computations for split virtqueues.
//!
//! SAFETY: This module is trusted to lay out the major virtqueue parts
//! (descriptor table, ring of submitted buffers, ring of returned buffers) into
//! a contiguous VMO.
//!
//! [`VirtioQueueMemoryLayout`] computes the layout,
//! [`VirtioQueuePhysicalMemoryLayout`] translates the layout into physical
//! memory addresses used by the PCI virtio device, [`VirtioQueuePartsBuilder`]
//! creates the structs that manage the parts.

use super::abi;
use super::descriptor_table::VirtioQueueDescriptorTable;
use super::rings::{VirtioQueueReturnedRing, VirtioQueueSubmittedRing};

use fuchsia_runtime;
use std::mem::align_of;
use std::num::NonZero;
use std::ptr::NonNull;
use zx::{VmarFlags, Vmo};

/// The computed memory layout of a split virtqueue.
pub struct VirtioQueueMemoryLayout {
    queue_capacity: NonZero<u16>,

    total_size: NonZero<usize>,

    submitted_ring_offset: usize,
    returned_ring_offset: usize,
}

// The smallest integer that is at least `offset` and divisible by `alignment`.
//
// `alignment` must be a power of two.
fn align_to(offset: usize, alignment: usize) -> usize {
    debug_assert!(alignment.is_power_of_two());

    // Mathematical equivalent of: alignment * offset.div_ceil(alignment)
    (offset + alignment - 1) & !(alignment - 1)
}

impl VirtioQueueMemoryLayout {
    /// The maximum number of descriptors in a queue.
    ///
    /// The value is a consequence of the constraints that the queue
    /// capacity must be a power of 2 and must be representable in a `u16`.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    // @cite(virtio): sec="2.8" title="Packed Virtqueues"
    pub const MAX_CAPACITY: u16 = 32768;

    /// Computes the memory layout given a queue capacity.
    ///
    /// `queue_capacity` must be a non-zero power of two, and must not exceed
    /// [`MAX_CAPACITY`]. `page_size` must be a non-zero power of two.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    pub fn new(queue_capacity: NonZero<u16>, page_size: NonZero<usize>) -> VirtioQueueMemoryLayout {
        debug_assert!(
            queue_capacity.is_power_of_two(),
            "Queue capacity not a power of two: {}",
            queue_capacity
        );
        debug_assert!(
            queue_capacity.get() <= Self::MAX_CAPACITY,
            "Queue capacity too large: {}",
            queue_capacity
        );
        debug_assert!(page_size.is_power_of_two(), "Page size not a power of two: {}", page_size);

        let descriptor_table_size = VirtioQueueDescriptorTable::size_bytes(queue_capacity);
        let submitted_ring_size = VirtioQueueSubmittedRing::size_bytes(queue_capacity);
        let returned_ring_size = VirtioQueueReturnedRing::size_bytes(queue_capacity);
        let submitted_ring_offset = descriptor_table_size.get();
        debug_assert!(
            submitted_ring_offset % align_of::<abi::SubmittedRingEntry>() == 0,
            "The descriptor table size is not a multiple of the submitted ring alignment"
        );

        // We may need padding between the submitted ring and the returned ring.
        //
        // The submitted ring's size (number of bytes in memory) is guaranteed
        // to be a multiple of two, but not guaranteed to be a multiple of 4.
        // The returned ring entries have a mandatory alignment of 4.
        let returned_ring_offset = align_to(submitted_ring_offset + submitted_ring_size.get(), 4);
        debug_assert!(
            returned_ring_offset % align_of::<abi::ReturnedRingEntry>() == 0,
            "Submitted ring padding computed incorrectly"
        );

        // The virtio ring metadata takes up a whole number of contiguous pages.
        let total_size = NonZero::<usize>::new(align_to(
            returned_ring_offset + returned_ring_size.get(),
            page_size.get(),
        ))
        .expect("total_size is zero");

        VirtioQueueMemoryLayout {
            queue_capacity,
            total_size,
            submitted_ring_offset,
            returned_ring_offset,
        }
    }

    /// The number of entries in the virtqueue.
    pub fn queue_capacity(&self) -> NonZero<u16> {
        self.queue_capacity
    }

    /// Points to the first byte in the descriptor table.
    ///
    /// Guaranteed to meet the table entries' alignment requirement. Guaranteed
    /// to not overlap with other queue parts (the rings of submitted and
    /// returned buffers).
    pub fn descriptor_table_offset(&self) -> usize {
        0
    }

    /// Points to the first byte in the ring of submitted buffers.
    ///
    /// Guaranteed to meet alignment requirements on all ring parts (header,
    /// entries, trailer). Guaranteed to not overlap with other queue parts (the
    /// descriptor table and the ring of returned buffers).
    pub fn submitted_ring_offset(&self) -> usize {
        self.submitted_ring_offset
    }

    /// Points to the first byte in the ring of submitted buffers.
    ///
    /// Guaranteed to meet alignment requirements on all ring parts (header,
    /// entries, trailer). Guaranteed to not overlap with the other queue parts
    /// (the descriptor table and the ring of submitted buffers).
    pub fn returned_ring_offset(&self) -> usize {
        self.returned_ring_offset
    }

    /// Number of bytes used by the split virtqueue data structures.
    ///
    /// Guaranteed to be a multiple of the page size passed to [`new()`].
    pub fn total_size(&self) -> NonZero<usize> {
        self.total_size
    }
}

/// Describes a split virtqueue's layout in physical memory.
///
/// The information is sufficient for registering the virtqueue with a virtio
/// device.
// @cite(virtio): sec="2.7" title="Split Virtqueues"
pub struct VirtioQueuePhysicalMemoryLayout {
    /// Physical address of the first byte in the buffer descriptor table.
    // @cite(virtio): sec="2.6" title="Virtqueues"
    // @cite(virtio): sec="2.7.5" title="The Virtqueue Descriptor Table"
    // @alias(virtio): theirs="Descriptor Area"
    pub descriptor_table_physical_address: u64,

    /// Physical address of the first byte in the submitted buffer ring.
    // @cite(virtio): sec="2.6" title="Virtqueues"
    // @cite(virtio): sec="2.7.6" title="The Virtqueue Available Ring"
    // @alias(virtio): theirs="Driver Area"
    pub submitted_ring_physical_address: u64,

    /// Physical address of the first byte in the returned buffer ring.
    // @cite(virtio): sec="2.6" title="Virtqueues"
    // @cite(virtio): sec="2.7.8" title="The Virtqueue Used Ring"
    // @alias(virtio): theirs="Device Area"
    pub returned_ring_physical_address: u64,
}

impl VirtioQueuePhysicalMemoryLayout {
    /// Computes the physical addresses needed by the virtio device.
    pub fn new(
        layout: &VirtioQueueMemoryLayout,
        start_physical_address: u64,
    ) -> VirtioQueuePhysicalMemoryLayout {
        VirtioQueuePhysicalMemoryLayout {
            descriptor_table_physical_address: start_physical_address
                + layout.descriptor_table_offset() as u64,
            submitted_ring_physical_address: start_physical_address
                + layout.submitted_ring_offset() as u64,
            returned_ring_physical_address: start_physical_address
                + layout.returned_ring_offset() as u64,
        }
    }
}

/// Allocates a virtqueue's backing memory to the queue's parts.
pub struct VirtioQueuePartsBuilder {
    queue_capacity: NonZero<u16>,

    /// Points to the start of the descriptor table.
    ///
    /// `None` after [`build_descriptor_table()`] is called.
    descriptors_ptr: Option<NonNull<abi::VirtioMemoryRangeDescriptor>>,

    /// Points to the start of the submitted ring header.
    ///
    /// `None` after [`build_submitted_ring()`] is called.
    submitted_ring_header_ptr: Option<NonNull<abi::SubmittedRingHeader>>,

    /// Points to the start of the returned ring header.
    ///
    /// `None` after [`build_returned_ring()`] is called.
    returned_ring_header_ptr: Option<NonNull<abi::ReturnedRingHeader>>,
}

impl VirtioQueuePartsBuilder {
    /// Allocates the memory needed to back a virtqueue.
    ///
    /// `queue_data_vmo` must have at least
    /// [`VirtioQueueMemoryLayout::size_bytes()`] bytes.
    pub fn new(
        queue_layout: &VirtioQueueMemoryLayout,
        queue_data_vmo: &Vmo,
    ) -> Result<VirtioQueuePartsBuilder, zx::Status> {
        // When debug assertions are disabled, [`zx::vmar::map()`] will fail
        // with an error if the VMO is too small.
        debug_assert!(
            queue_data_vmo.get_size().unwrap() >= queue_layout.total_size.get() as u64,
            "VMO has {} bytes, virtqueue data structures require {} bytes",
            queue_data_vmo.get_size().unwrap(),
            queue_layout.total_size.get(),
        );

        let queue_data_address = fuchsia_runtime::vmar_root_self()
            .map(
                /* vmar_offset= */ 0,
                &queue_data_vmo,
                /* vmo_offset= */ 0,
                queue_layout.total_size.get(),
                VmarFlags::PERM_READ | VmarFlags::PERM_WRITE | VmarFlags::REQUIRE_NON_RESIZABLE,
            )
            .map_err(|_| zx::Status::INTERNAL)?;
        let queue_data_address = NonZero::<usize>::new(queue_data_address)
            .expect("zx_vmar_map() returned null pointer on success");
        let queue_data = NonNull::<u8>::with_exposed_provenance(queue_data_address);

        // SAFETY: `queue_data` points into `queue_data_vmo`, whose size fits
        // the virtqueue's data structures. [`zx::vmar::map()`] causes the root
        // VMAR to retain a reference to the VMO, so the memory will remain
        // allocated, even as `queue_data_vmo` gets out of scope. [`VirtioQueue`]
        unsafe { Ok(Self::from_owned_memory(queue_layout, queue_data)) }
    }

    /// Computes pointers to all virtqueue regions.
    ///
    /// SAFETY: `queue_data` must point to
    /// [`VirtioQueueMemoryLayout::total_size`] bytes that belong to the same
    /// memory allocation. The rest of the driver may not access the memory
    /// range after handing it to this method.
    unsafe fn from_owned_memory(
        layout: &VirtioQueueMemoryLayout,
        queue_data: NonNull<u8>,
    ) -> VirtioQueuePartsBuilder {
        // SAFETY: `queue_data` preconditions guarantee that all queue parts are
        // in the same memory allocation.
        let descriptors_ptr = unsafe { queue_data.byte_add(layout.descriptor_table_offset()) };

        debug_assert!(
            (descriptors_ptr.as_ptr() as usize) % align_of::<abi::VirtioMemoryRangeDescriptor>()
                == 0
        );
        let descriptors_ptr = descriptors_ptr.cast::<abi::VirtioMemoryRangeDescriptor>();

        // SAFETY: `queue_data` preconditions guarantee that all queue parts are
        // in the same memory allocation.
        let submitted_ring_header_ptr =
            unsafe { queue_data.byte_add(layout.submitted_ring_offset()) };
        debug_assert!(
            (submitted_ring_header_ptr.as_ptr() as usize) % align_of::<abi::SubmittedRingHeader>()
                == 0
        );
        let submitted_ring_header_ptr =
            submitted_ring_header_ptr.cast::<abi::SubmittedRingHeader>();

        // SAFETY: `queue_data` preconditions guarantee that all queue parts are
        // in the same memory allocation.
        let returned_ring_header_ptr =
            unsafe { queue_data.byte_add(layout.returned_ring_offset()) };

        debug_assert!(
            (returned_ring_header_ptr.as_ptr() as usize) % align_of::<abi::ReturnedRingHeader>()
                == 0
        );
        let returned_ring_header_ptr = returned_ring_header_ptr.cast::<abi::ReturnedRingHeader>();

        VirtioQueuePartsBuilder {
            queue_capacity: layout.queue_capacity(),
            descriptors_ptr: Some(descriptors_ptr),
            submitted_ring_header_ptr: Some(submitted_ring_header_ptr),
            returned_ring_header_ptr: Some(returned_ring_header_ptr),
        }
    }

    /// Returns the manager of the queue's descriptor table memory.
    ///
    /// Panics if called more than once on the same parts instance.
    pub fn build_descriptor_table(&mut self) -> VirtioQueueDescriptorTable {
        let table_ptr =
            self.descriptors_ptr.take().expect("build_descriptor_table() already called");

        // SAFETY: The pointer to the table's memory will only be produced once,
        // because the optional holding it is reset to [`None`] above. The
        // [`VirtioQueueMemoryLayout`] implementation in this module is trusted
        // to produce correct offsets, so the queue parts use non-overlapping
        // parts of the VMO allocated in [`new()`].
        unsafe { VirtioQueueDescriptorTable::new(table_ptr, self.queue_capacity) }
    }

    /// Returns the manager of the queue's submitted ring memory.
    ///
    /// Panics if called more than once on the same parts instance.
    pub fn build_submitted_ring(&mut self) -> VirtioQueueSubmittedRing {
        let ring_ptr =
            self.submitted_ring_header_ptr.take().expect("build_submitted_ring() already called");

        // SAFETY: The pointer to the ring's memory will only be produced once,
        // because the optional holding it is reset to [`None`] above. The
        // [`VirtioQueueMemoryLayout`] implementation in this module is trusted
        // to produce correct offsets, so the queue parts use non-overlapping
        // parts of the VMO allocated in [`new()`].
        unsafe { VirtioQueueSubmittedRing::new(ring_ptr, self.queue_capacity) }
    }

    /// Returns the manager of the queue's returned ring memory.
    ///
    /// Panics if called more than once on the same parts instance.
    pub fn build_returned_ring(&mut self) -> VirtioQueueReturnedRing {
        let ring_ptr =
            self.returned_ring_header_ptr.take().expect("build_returned_ring() already called");

        // SAFETY: The pointer to the ring's memory will only be produced once,
        // because the optional holding it is reset to [`None`] above. The
        // [`VirtioQueueMemoryLayout`] implementation in this module is trusted
        // to produce correct offsets, so the queue parts use non-overlapping
        // parts of the VMO allocated in [`new()`].
        unsafe { VirtioQueueReturnedRing::new(ring_ptr, self.queue_capacity) }
    }
}

// TODO: VirtioQueueMemoryLayout tests -- page size 4096 capacity 1, page size 16384
// capacity 1, page size 4096 capacity 64, page size 16384 capacity 64, page size 4096
// capacity 32768

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    #[allow(clippy::absurd_extreme_comparisons)]
    fn test_descriptor_list_sentinel() {
        assert!(
            VirtioQueueDescriptorTable::SENTINEL >= VirtioQueueMemoryLayout::MAX_CAPACITY,
            "The sentinel overlaps the set of valid descriptor indices"
        );
    }
}
