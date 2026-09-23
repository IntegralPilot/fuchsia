// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! virtqueue descriptor table management

use super::abi;
use super::buffer::{VirtioMemoryRange, VirtioMemoryRangeAccess, VirtioSubmittedBufferId};

use std::marker::PhantomData;
use std::mem::offset_of;
use std::num::NonZero;
use std::ptr::NonNull;

/// Descriptor table index guaranteed to be valid.
///
/// The descriptor table has the same capacity throughout its lifetime. So, once a
/// table index is valid, it remains valid.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VirtioQueueDescriptorTableIndex(u16);

impl VirtioQueueDescriptorTableIndex {
    pub fn new(value: u16) -> VirtioQueueDescriptorTableIndex {
        assert!(value != VirtioQueueDescriptorTable::SENTINEL);
        VirtioQueueDescriptorTableIndex(value)
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Points to the beginning of a list of descriptors.
///
/// Instances can be obtained from
/// [`VirtioQueueDescriptorTable::take_descriptors()`].
///
/// Owning an instance implies ownership over the buffer descriptors in the
/// list.
pub struct VirtioQueueDescriptorListHead {
    pub(crate) first_index: VirtioQueueDescriptorTableIndex,
}

impl VirtioQueueDescriptorListHead {
    fn new(first_index: u16) -> VirtioQueueDescriptorListHead {
        let first_index = VirtioQueueDescriptorTableIndex::new(first_index);

        VirtioQueueDescriptorListHead { first_index }
    }

    pub fn first_index(&self) -> VirtioQueueDescriptorTableIndex {
        self.first_index
    }
}

impl From<&VirtioQueueDescriptorListHead> for VirtioSubmittedBufferId {
    fn from(submitted_buffer_id: &VirtioQueueDescriptorListHead) -> VirtioSubmittedBufferId {
        VirtioSubmittedBufferId(submitted_buffer_id.first_index.value())
    }
}

/// Manages a virtqueue's descriptor table.
///
/// The API and implementation work around the fact that the Rust memory model
/// does not allow creating a mutable reference to any descriptor table entry,
/// because the virtio device may read the descriptor table at any time.
///
/// Owning an instance is conceptually equivalent to owning the managed array of
/// descriptor table entries. So, at most one instance may exist for a
/// virtqueue.
// @cite(virtio): sec="2.7.5" title="The Virtqueue Descriptor Table"
pub struct VirtioQueueDescriptorTable {
    /// Points to the start of the managed descriptor table.
    descriptors: NonNull<abi::VirtioMemoryRangeDescriptor>,

    /// The number of entries in the table.
    capacity: NonZero<u16>,

    /// Index of the first free descriptor in the descriptor table.
    ///
    /// Set to [`VirtioQueueDescriptorTable::SENTINEL`] when the table is full.
    next_free_index: u16,

    /// This struct owns the table entries' memory.
    ///
    /// All ring entries point to valid indices in the associated virtqueue's
    /// descriptor table.
    ///
    /// Every entry's [`abi::VirtioMemoryRangeDescriptor::next_descriptor_index`] is
    /// set to either a descriptor index within the table's bounds, or to
    /// [`VirtioQueueDescriptorTable::SENTINEL`].
    ///
    /// The `next_descriptor_index` values effectively describe a set of
    /// disjoint single linked lists of descriptors, where [`next_free_index`]
    /// points to the start of the list of free descriptors.
    _owns_entries: PhantomData<[abi::VirtioMemoryRangeDescriptor]>,
}

/// SAFETY: The struct owns the memory that it accesses, so it is conceptually
/// equivalent to any other Rust type that owns its data.
unsafe impl Send for VirtioQueueDescriptorTable {}

impl VirtioQueueDescriptorTable {
    /// `descriptors` must meet the requirement that the memory used by the
    /// queue's descriptor table must be zeroed.
    ///
    /// SAFETY: `descriptors` must point to [`size_bytes()`] bytes that belong
    /// to the same memory allocation. The rest of the driver must not access
    /// the memory range after handing it to this method.
    // @cite(virtio): sec="4.1.5.1.3" title="Virtqueue Configuration"
    pub unsafe fn new(
        descriptors: NonNull<abi::VirtioMemoryRangeDescriptor>,
        capacity: NonZero<u16>,
    ) -> VirtioQueueDescriptorTable {
        let mut descriptor_table = VirtioQueueDescriptorTable {
            descriptors,
            capacity,
            next_free_index: 0,

            // [`zerocopy::FromBytes`] guarantees that zeroed memory makes up
            // valid [`abi::VirtioMemoryRangeDescriptor`] values. In addition, zero
            // is a valid index for every descriptor table (virtqueue capacity
            // is at least 1).
            _owns_entries: PhantomData,
        };
        descriptor_table.initialize_free_list(capacity);

        descriptor_table
    }

    /// "next descriptor index" value that signals the end of the free list.
    ///
    /// Guaranteed not to overlap with the set of valid descriptor indices.
    pub const SENTINEL: u16 = 0xffff;

    /// Computes the pointer to a descriptor entry.
    ///
    /// The returned pointer is guaranteed to point to a valid
    /// [`abi::VirtioMemoryRangeDescriptor`] place.
    ///
    /// SAFETY: `index` must be smaller than the queue capacity.
    unsafe fn descriptor(&self, index: u16) -> NonNull<abi::VirtioMemoryRangeDescriptor> {
        debug_assert!(
            index < self.capacity.get(),
            "Descriptor table index out of bounds: {}",
            index
        );

        // SAFETY: Follows from the method's safety preconditions.
        unsafe { self.descriptors.add(index as usize) }
    }

    /// Computes the pointer to a descriptor entry's "next descriptor" index.
    ///
    /// The returned pointer is guaranteed to point to a valid
    /// [`abi::VirtioMemoryRangeDescriptor::next_descriptor_index`] place.
    ///
    /// SAFETY: `index` must be smaller than the queue capacity.
    unsafe fn descriptor_next_index(&self, index: u16) -> NonNull<u16> {
        debug_assert!(
            index < self.capacity.get(),
            "Descriptor table index out of bounds: {}",
            index
        );

        // SAFETY: Follows from the method's safety preconditions.
        let descriptor = unsafe { self.descriptor(index) };

        let byte_ptr = descriptor.cast::<u8>();
        // SAFETY: The offset is within the struct bounds.
        let next_index_byte_ptr = unsafe {
            byte_ptr.add(offset_of!(abi::VirtioMemoryRangeDescriptor, next_descriptor_index))
        };
        next_index_byte_ptr.cast::<u16>()
    }

    /// Reads a descriptor table entry's next descriptor index.
    ///
    /// The returned value is guaranteed to be either a valid descriptor index
    /// or [`SENTINEL`].
    ///
    /// SAFETY: `index` must be smaller than the queue capacity.
    unsafe fn read_descriptor_next_index_unchecked(&self, index: u16) -> u16 {
        debug_assert!(
            index < self.capacity.get(),
            "Descriptor table index out of bounds: {}",
            index
        );

        // SAFETY: [`descriptor_next_index()`] is guaranteed to return a valid
        // pointer while the queue's memory is mapped. The descriptor table is
        // not modified by the device, so it can be read directly.
        let next_descriptor_index = unsafe { *self.descriptor_next_index(index).as_ptr() };

        debug_assert!(
            next_descriptor_index == Self::SENTINEL || next_descriptor_index < self.capacity.get(),
            "Descriptor table has invalid next_descriptor_index: {}",
            next_descriptor_index,
        );

        next_descriptor_index
    }

    /// Sets a descriptor table entry's next descriptor index.
    ///
    /// SAFETY: `index` must be [`SENTINEL`], or must be smaller than the queue
    /// capacity. `index` must not create a cycle in the set of lists defined by
    /// the next descriptor index.
    unsafe fn set_descriptor_next_index_unchecked(
        &mut self,
        index: u16,
        next_descriptor_index: u16,
    ) {
        debug_assert!(
            index < self.capacity.get(),
            "Descriptor table index out of bounds: {}",
            index
        );
        debug_assert!(
            next_descriptor_index == Self::SENTINEL || next_descriptor_index < self.capacity.get(),
            "Invalid next_descriptor_index: {}",
            next_descriptor_index,
        );

        // SAFETY: [`descriptor_next_index()`] is guaranteed to return a valid
        // pointer while the queue's memory is mapped.
        unsafe {
            std::ptr::write_volatile(
                self.descriptor_next_index(index).as_ptr(),
                next_descriptor_index,
            );
        }
    }

    /// Sets up the entire descriptor table as a freelist.
    ///
    /// The specification mandates that the memory used by the queue's descriptor
    /// table must be zeroed. We deviate from this recommendation, because we
    /// implement a free list in the descriptor table memory's memory. This
    /// deviation is not observable by any device that only reads the table
    /// entries referenced in the submitted ring. This deviation is known to work
    /// on the emulators targeted by this driver.
    // @cite(virtio): sec="4.1.5.1.3" title="Virtqueue Configuration"
    fn initialize_free_list(&mut self, capacity: NonZero<u16>) {
        let capacity_u16 = capacity.get();
        for index in 0..capacity_u16 {
            let next_index_value =
                if index == capacity_u16 - 1 { Self::SENTINEL } else { index + 1 };

            // SAFETY: `index` iterates over the set of valid indices.
            unsafe {
                self.set_descriptor_next_index_unchecked(index, next_index_value);
            }
        }
    }

    /// Reads a descriptor table entry's next descriptor index.
    ///
    /// Returns [`None`] if the table entry does not point to another
    /// descriptor.
    pub fn read_descriptor_next_index(
        &self,
        index: VirtioQueueDescriptorTableIndex,
    ) -> Option<VirtioQueueDescriptorTableIndex> {
        assert!(
            index.0 < self.capacity.get(),
            "VirtioQueueDescriptorTableIndex contains out-of-bounds index: {}",
            index.0,
        );

        let next_index = unsafe { self.read_descriptor_next_index_unchecked(index.0) };
        if next_index == Self::SENTINEL {
            return None;
        }
        Some(VirtioQueueDescriptorTableIndex(next_index))
    }

    /// Sets a descriptor table entry's values.
    ///
    /// The function does not set the [`next_descriptor_index`] or the
    /// corresponding flag. Those are managed internally.
    pub fn set_descriptor(
        &mut self,
        index: VirtioQueueDescriptorTableIndex,
        range: &VirtioMemoryRange,
    ) {
        assert!(
            index.0 < self.capacity.get(),
            "VirtioQueueDescriptorTableIndex contains out-of-bounds index: {}",
            index.0,
        );

        // SAFETY: `index.0` is guaranteed to be valid by its enclosing type.
        let next_descriptor_index = unsafe { self.read_descriptor_next_index_unchecked(index.0) };

        let mut flags = abi::VirtioMemoryRangeFlags::default();
        flags.set_written_by_hardware(range.device_access() == VirtioMemoryRangeAccess::Output);
        flags.set_next_descriptor_index_is_valid(next_descriptor_index != Self::SENTINEL);

        let table_entry = abi::VirtioMemoryRangeDescriptor {
            physical_address: range.start_physical_address(),
            size_bytes: range.size().get(),
            flags,
            next_descriptor_index,
        };

        // SAFETY: `index` is guaranteed to be valid due to its type.
        unsafe {
            std::ptr::write_volatile(self.descriptor(index.0).as_ptr(), table_entry);
        }
    }

    /// Pops `count` descriptors from the free list.
    ///
    /// Returns the index of the first popped descriptor. The following
    /// descriptors are linked into a list using the "next descriptor index"
    /// pointers. This intentionally matches the expected representation of
    /// the descriptor list used to encode a buffer's memory ranges.
    ///
    /// Returns [`None`] if the free list does not contain enough descriptors.
    /// The free list remains unchanged in this case.
    pub fn take_descriptors(
        &mut self,
        count: NonZero<u16>,
    ) -> Option<VirtioQueueDescriptorListHead> {
        // The maximum possible descriptor chain length is the queue size.
        // @cite(virtio): sec="2.7.5" title="The Virtqueue Descriptor Table"
        debug_assert!(count.get() <= self.capacity.get());

        // To facilitate simpler internal interfaces, we deviate from the
        // reference code for placing buffers into the descriptor table.
        //
        // The reference code fills out each descriptor as it gets removed from
        // the free list. We first remove all the descriptors from the free
        // list, and then populate them.
        //
        // The deviation is not visible to a device that only reads descriptor
        // entries offered via the submitted ring, because we populate all the
        // descriptors before adding an entry to the submitted ring.
        // @cite(virtio): sec="2.7.13.1" title="Placing Buffers Into The Descriptor Table"

        // The head of the allocated list of descriptors will be returned.
        let first_allocated_index = self.next_free_index;

        // Traverse candidate descriptors to verify that `count` descriptors are
        // available before modifying `self.next_free_index`.
        // @cite(virtio): sec="2.7.13.1" title="Placing Buffers Into The Descriptor Table"
        let mut current = self.next_free_index;
        let mut last_allocated_index = Self::SENTINEL;

        for _ in 0..count.get() {
            if current == Self::SENTINEL {
                return None;
            }
            last_allocated_index = current;
            // SAFETY: `current` is guaranteed to be a valid descriptor index (< capacity).
            current = unsafe { self.read_descriptor_next_index_unchecked(current) };
        }

        self.next_free_index = current;

        debug_assert!(
            last_allocated_index != Self::SENTINEL,
            "Incorrect list traversal logic above"
        );
        // We must invalidate the last descriptor's `next_descriptor_index` to
        // separate the returned list from the free list. The returned list's
        // sentinel will allow us to join the returned list back into the free
        // list later.
        // SAFETY: `last_allocated_index` is read from the descriptor table.
        // It is guaranteed not to be [`SENTINEL`], so it must be a valid index.
        unsafe {
            self.set_descriptor_next_index_unchecked(last_allocated_index, Self::SENTINEL);
        }

        Some(VirtioQueueDescriptorListHead::new(first_allocated_index))
    }

    /// Prepends a list of returned descriptors to the free list.
    ///
    /// `list_head` must be returned from [`take_descriptors()`].
    pub fn return_descriptors(&mut self, list_head: VirtioQueueDescriptorListHead) {
        debug_assert!(
            list_head.first_index.value() < self.capacity.get(),
            "List head points past the end of the descriptor table"
        );

        let first_returned_index = list_head.first_index;

        // Tracks the descriptor table entry that will be connected to the free
        // list.
        let mut last_returned_index: VirtioQueueDescriptorTableIndex;

        let mut next_returned_index = first_returned_index;
        loop {
            last_returned_index = next_returned_index;

            match self.read_descriptor_next_index(next_returned_index) {
                Some(index) => {
                    next_returned_index = index;
                }
                None => {
                    break;
                }
            }
        }

        unsafe {
            self.set_descriptor_next_index_unchecked(
                last_returned_index.value(),
                self.next_free_index,
            );
        }
        self.next_free_index = first_returned_index.value();
    }

    /// The number of bytes needed by the table.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    pub fn size_bytes(capacity: NonZero<u16>) -> NonZero<usize> {
        NonZero::<usize>::new(
            size_of::<abi::VirtioMemoryRangeDescriptor>() * (capacity.get() as usize),
        )
        .expect("Incorrect descriptor table size computation")
    }
}
