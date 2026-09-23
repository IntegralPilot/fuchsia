// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Models buffers used in virtqueues.

// TODO(https://fxbug.dev/504722357): Remove this in favor of more granular
// attributes when the Rust port is completed.
#![allow(dead_code)]

use std::num::NonZero;

/// Describes a buffer that can be submitted to a virtio queue.
///
/// A buffer describes the memory used by a virtio device to read the input and
/// write the output for a task. The description is a list of
/// [`VirtioMemoryRange`]. Each memory range must be contiguous in the PCI
/// device's addressable space.
///
/// The driver submits a buffer to ask the virtio device to perform a task.
/// Submitting a buffer transfers the driver's ownership of the buffer to the
/// device. The device returns the buffer when it completes the task. Returning
/// the buffer transfers ownership back to the driver.
///
/// For convenience, we say that a buffer is submitted when it is owned by the
/// device.  We also say that a memory range is submitted when the buffer that
/// it belongs to is submitted. The device may only access the memory submitted
/// buffers, which are the buffers that it owns.
///
/// The list of memory regions must first contain the regions storing the task's
/// input (using [`VirtioMemoryRangeAccess::Input`]), followed by the regions
/// storing the task's output (using [`VirtioMemoryRangeAccess::Output`]).
// @cite(virtio): sec="2.7" title="Split Virtqueues"
// @cite(virtio): sec="2.7.4" title="Message Framing"
// @cite(virtio): sec="2.7.4.2" title="Driver Requirements: Message Framing"
// @cite(virtio): sec="2.7.5" title="The Virtqueue Descriptor Table"
// @cite(virtio): sec="2.7.13.1" title="Placing Buffers Into The Descriptor Table"
// @cite(virtio): sec="2.8" title="Packed Virtqueues"
pub struct VirtioBufferRef<'a> {
    range_list: &'a [VirtioMemoryRange],
}

impl<'a> AsRef<[VirtioMemoryRange]> for VirtioBufferRef<'a> {
    fn as_ref(&self) -> &[VirtioMemoryRange] {
        self.range_list
    }
}

impl<'a> VirtioBufferRef<'a> {
    /// The maximum number of contiguous memory regions in a buffer.
    ///
    /// The upper bound is based on the fact that a buffer's memory regions must
    /// fit in a virtqueue's descriptor table, so the number of memory regions
    /// is capped by the virtqueue descriptor table size.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    // @cite(virtio): sec="2.8" title="Packed Virtqueues"
    pub const MAX_LENGTH: usize = 32768;

    /// Panics if the list of ranges is empty, or if a range with
    /// [`VirtioMemoryRangeAccess::Input`] access follows a range with
    /// [`VirtioMemoryRangeAccess::Output`] access.
    pub fn new(range_list: &'a [VirtioMemoryRange]) -> VirtioBufferRef<'a> {
        assert!(!range_list.is_empty(), "virtio buffers must have at least one memory range");
        assert!(
            range_list.len() <= Self::MAX_LENGTH,
            "virtio buffer has too many memory ranges: {}",
            range_list.len()
        );

        // Check the memory access constraint.
        let mut last_device_access = VirtioMemoryRangeAccess::Input;
        for range in range_list {
            if range.device_access == last_device_access {
                continue;
            }
            assert!(
                range.device_access == VirtioMemoryRangeAccess::Output,
                "Range with {:?} access following range with {:?} access",
                range.device_access,
                last_device_access,
            );
            last_device_access = range.device_access;
        }

        VirtioBufferRef { range_list }
    }

    /// Returns the number of memory ranges in the buffer.
    pub fn len(&self) -> NonZero<u16> {
        // `as` does not truncate the length because the constructor ensures the
        // range list has at most [`Self::MAX_LENGTH`] ranges.
        // [`Option::unwrap()`] will not panic because the constructor ensures
        // that the range list is not empty.
        NonZero::<u16>::new(self.range_list.len() as u16).unwrap()
    }
}

/// Identifies a buffer submitted to a virtqueue.
///
/// The identifier uniquely identifies a buffer from the moment it is submitted
/// to the device via the queue until the moment it is returned to the driver
/// via the queue. Once the buffer is returned to the driver, a future queue
/// submission may reuse the identifier. The identifier is unique within one
/// virtqueue.
///
/// It follows that higher-level code may use the buffer ID (potentially
/// qualified by the virtqueue index) to track other buffer-related resources,
/// as long as the tracking stops right after a buffer is returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtioSubmittedBufferId(pub u16);

/// Describes a buffer returned by a virtqueue.
pub struct VirtioReturnedBufferInfo {
    /// Indicates which previously submitted buffer was returned.
    ///
    /// [`VirtioSubmittedBufferId`] documents the validity window.
    pub submitted_buffer_id: VirtioSubmittedBufferId,

    /// The number of bytes written by the device to the buffer.
    ///
    /// The device writes to the buffer regions that allow it by having their
    /// [`VirtioMemoryRangeFlags::written_by_hardware`] set to true. The device
    /// uses the writable regions in the order in which they appear in the
    /// submitted buffer region list.
    // @cite(virtio): sec="2.7.8.2" title="Device Requirements: The Virtqueue Used Ring"
    pub written_bytes: u32,
}

/// Describes a memory region accessible by a virtio device.
///
/// The memory region is contiguous in the PCI device's physical address space.
/// If no IOMMU is involved, the PCI device's physical address space is the same
/// as the CPU physical address space visible to the guest (virtualized) OS.
///
/// Each instance describes a memory region that is submitted (owned by the
/// device) or is about to be submitted (owned by the device). Device ownership
/// is conceptually equivalent to a Rust mutable reference covering the entire
/// memory range. Here are two immediate implications:
///
/// 1. No Rust references may point into a range described by an instance.
/// 2. The memory ranges described by any two instances must not overlap.
// @cite(virtio): sec="2.7.5" title="The Virtqueue Descriptor Table"
// @cite(virtio): sec="2.7.13.1" title="Placing Buffers Into The Descriptor Table"
#[derive(Debug)]
pub struct VirtioMemoryRange {
    start_physical_address: u64,
    size: NonZero<u32>,
    device_access: VirtioMemoryRangeAccess,
}

impl VirtioMemoryRange {
    /// SAFETY: `start_physical_address` and `size` must identify a valid range
    /// of memory that is contiguous in the PCI virtio device's physical address
    /// space. The newly created instance must be treated like a mutable Rust
    /// reference covering the entire memory range.
    pub unsafe fn new(
        start_physical_address: u64,
        size: NonZero<u32>,
        device_access: VirtioMemoryRangeAccess,
    ) -> VirtioMemoryRange {
        VirtioMemoryRange { start_physical_address, size, device_access }
    }

    /// Points to the first byte in the memory region.
    ///
    /// Uses the PCI device's physical addressable space.
    pub fn start_physical_address(&self) -> u64 {
        self.start_physical_address
    }

    /// The number of bytes that make up the contiguous region.
    pub fn size(&self) -> NonZero<u32> {
        self.size
    }

    /// Constrains a device's accesses to the memory range.
    pub fn device_access(&self) -> VirtioMemoryRangeAccess {
        self.device_access
    }
}

/// Describes the way a device may access a [`VirtioMemoryRange`].
///
/// The virtio device may only access a range's memory while the range belongs
/// to a buffer that is submitted (owned by the device).
// @cite(virtio): sec="2.7.5" title="The Virtqueue Descriptor Table"
// @alias(virtio): theirs="VIRTQ_DESC_F_WRITE"
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtioMemoryRangeAccess {
    /// The device may read from the memory range while owning its buffer.
    ///
    /// This access mode implies that the memory range stores input parameters
    /// for the task assigned to the device.
    ///
    /// The driver is expected to initialize (write) all the range's memory
    /// before submitting the buffer to the device.
    ///
    /// The Rust memory model dictates that the driver may hold immutable
    /// references to the buffer's memory while the buffer is submitted.
    /// However, for simplicity, we use the stronger constraint in [`Output`]
    /// for all ranges owned by the device.
    // @cite(virtio): sec="2.7.5" title="The Virtqueue Descriptor Table"
    // @alias(virtio): theirs="device-readable"
    Input,

    /// The device may write to the memory range while owning its buffer.
    ///
    /// This access mode implies that the memory range stores the output (also
    /// called "results" and "returned values") of the task assigned to the
    /// device.
    ///
    /// The driver is expected to read the output from the range after the
    /// device returns it. The Rust memory model dictates that the driver must
    /// not have any reference to the range's memory while the range's buffer is
    /// owned by the device.
    // @cite(virtio): sec="2.7.5" title="The Virtqueue Descriptor Table"
    // @alias(virtio): theirs="device-writable"
    Output,
}
