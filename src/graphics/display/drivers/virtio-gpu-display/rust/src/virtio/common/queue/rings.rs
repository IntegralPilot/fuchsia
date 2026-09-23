// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Split virtqueue ring management.

// TODO(https://fxbug.dev/504722357): Remove this in favor of more granular
// attributes when the Rust port is completed.
#![allow(dead_code)]

use super::abi;
use super::buffer::VirtioReturnedBufferInfo;
use super::descriptor_table::{VirtioQueueDescriptorListHead, VirtioQueueDescriptorTableIndex};

use std::marker::PhantomData;
use std::mem::align_of;
use std::num::{NonZero, Wrapping};
use std::ptr::NonNull;
use std::sync::atomic::{self, Ordering};

/// Manages a virtqueue's ring of submitted buffers.
///
/// The API and implementation work around the fact that the Rust memory model
/// does not allow creating a mutable reference to any part of the submtted
/// ring, because the virtio device may read the ring at any time.
///
/// The instance's owner is the exclusive manager of the memory backing all the
/// ring's parts (header, array of ring entries, and trailer). So, at most one
/// instance may exist for a virtqueue.
// @cite(virtio): sec="2.7.6" title="The Virtqueue Available Ring"
pub struct VirtioQueueSubmittedRing {
    /// Points to the ring's header.
    ///
    /// The split virtqueue ABI implies that the header pointer implicitly
    /// indicates the location of the other ring parts (entries, footer). All
    /// ring parts belong to the same memory allocation.
    header: NonNull<abi::SubmittedRingHeader>,

    /// AND mask that maps an insertion counter to a ring index.
    ///
    /// The result of `insertion_counter & capacity_mask` is guaranteed to
    /// equal the result of `insertion_counter % capacity` for all valid
    /// values of `insertion_counter`.
    capacity_mask: u16,

    /// This struct owns the ring header's memory.
    _owns_header: PhantomData<abi::SubmittedRingHeader>,

    /// This struct owns the ring entries' memory.
    ///
    /// All ring entries point to valid indices in the associated virtqueue's
    /// descriptor table.
    _owns_entries: PhantomData<[abi::SubmittedRingEntry]>,

    /// This struct owns the ring trailer's memory.
    _owns_trailer: PhantomData<abi::SubmittedRingTrailer>,
}

/// SAFETY: The struct owns the memory that it accesses, so it is conceptually
/// equivalent to any other Rust type that owns its data.
unsafe impl Send for VirtioQueueSubmittedRing {}

impl VirtioQueueSubmittedRing {
    /// `ring_header` must point to zeroed memory.
    ///
    /// `capacity` is named "Queue Size" in the specification. The value must be a
    /// power of two.
    ///
    /// SAFETY: `ring_header` must point to [`size_bytes()`] bytes that belong
    /// to the same memory allocation. The rest of the driver must not access
    /// the memory range after handing it to this method.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    // @cite(virtio): sec="4.1.5.1.3" title="Virtqueue Configuration"
    // @alias(virtio): theirs="Queue Size"
    pub unsafe fn new(
        ring_header: NonNull<abi::SubmittedRingHeader>,
        capacity: NonZero<u16>,
    ) -> VirtioQueueSubmittedRing {
        debug_assert!(
            (ring_header.as_ptr() as usize) % align_of::<abi::SubmittedRingHeader>() == 0
        );
        debug_assert!(
            capacity.is_power_of_two(),
            "Virtio split queue capacity is not a power of two: {}",
            capacity,
        );

        VirtioQueueSubmittedRing {
            header: ring_header,
            capacity_mask: capacity.get() - 1,

            // [`zerocopy::FromBytes`] guarantees that zeroed memory is a valid
            // [`abi::SubmittedRingHeader`] value.
            _owns_header: PhantomData,

            // [`zerocopy::FromBytes`] guarantees that zeroed memory makes up
            // valid [`abi::SubmittedRingEntry`] values. In addition, zero is a valid
            // index for every descriptor table (virtqueue capacity is at least
            // 1).
            _owns_entries: PhantomData,

            // [`zerocopy::FromBytes`] guarantees that zeroed memory is a valid
            // [`abi::SubmittedRingTrailer`] value.
            _owns_trailer: PhantomData,
        }
    }

    /// Appends a buffer to the ring.
    ///
    /// `descriptor_list_head` describes the buffer's memory as a list of
    /// [`VirtioMemoryRange`].
    ///
    /// Uses a memory barrier that ensures all previous writes are posted to
    /// RAM. This is always necessary, because the virtio device is free to pop
    /// entries from the ring at any time.
    ///
    /// The caller is responsible for issuing a memory barrier and notifying the
    /// device that the virtqueue was modified. The specification encourages
    /// using a single notification for multiple modifications (batching) when
    /// possible.
    // @cite(virtio): sec="2.7.13" title="Supplying Buffers to The Device"
    // @cite(virtio): sec="2.8.21.1" title="Placing Available Buffers Into The Descriptor Ring"
    // @alias(virtio): theirs="batching"
    pub fn push_back(&mut self, descriptor_list_head: VirtioQueueDescriptorListHead) {
        // @cite(virtio): sec="2.7.13" title="Supplying Buffers to The Device"
        let entry = abi::SubmittedRingEntry {
            first_descriptor_index: descriptor_list_head.first_index().value(),
        };

        let insertion_counter = self.read_insertion_counter();
        let entry_ptr = self.entry_ptr(insertion_counter);

        // SAFETY: `entry_ptr` is guaranteed to be valid by construction.
        unsafe {
            std::ptr::write_volatile(entry_ptr.as_ptr(), entry);
        }

        let new_insertion_counter = insertion_counter + Wrapping(1);

        // This barrier ensures that all the memory writes to the virtqueue data
        // structure are visible to the device before the counter update. So,
        // the device sees valid virtqueue data any time it decides to poll the
        // queue for new entries.
        atomic::fence(Ordering::Release);

        self.set_insertion_counter(new_insertion_counter);
    }

    /// The number of bytes needed by a ring's data structures.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    pub fn size_bytes(capacity: NonZero<u16>) -> NonZero<usize> {
        NonZero::<usize>::new(
            size_of::<abi::SubmittedRingHeader>()
                + size_of::<abi::SubmittedRingEntry>() * (capacity.get() as usize)
                + size_of::<abi::SubmittedRingTrailer>(),
        )
        .expect("Incorrect size_bytes computation for the submitted ring")
    }

    /// Computes the pointer to a ring entry.
    ///
    /// `counter` must be the ring's insertion counter or extraction counter.
    /// Returns a pointer to the ring entry used by the operation.
    fn entry_ptr(&self, counter: Wrapping<u16>) -> NonNull<abi::SubmittedRingEntry> {
        // SAFETY: All the ring components (header, entries, trailer) belong to
        // the same memory allocation.
        let entries_ptr = unsafe { self.header.as_ptr().add(1) }.cast::<abi::SubmittedRingEntry>();

        // [`Option::unwrap()`] will not panic because `entries_ptr` was
        // computed by adding a [`usize`] to a [`NonNull`] pointer, and the
        // result of the addition is guaranteed to be non-null.
        let entries = NonNull::new(entries_ptr).unwrap();
        debug_assert!((entries.as_ptr() as usize) % align_of::<abi::SubmittedRingEntry>() == 0);

        let entry_index = self.entry_index_from_counter(counter);

        // SAFETY: `entry_index` points to a valid ring entry. All ring entries
        // belong to the same memory allocation.
        unsafe { entries.add(entry_index as usize) }
    }

    /// Computes the ring entry index corresponding to an operation counter.
    ///
    /// `counter` must be the ring's insertion counter or extraction counter.
    /// The returned value is the index of the ring entry used by the operation.
    fn entry_index_from_counter(&self, counter: Wrapping<u16>) -> u16 {
        counter.0 & self.capacity_mask
    }

    /// Reads the insertion counter from the header.
    fn read_insertion_counter(&self) -> Wrapping<u16> {
        // SAFETY: The header pointer is valid while the queue's memory is
        // mapped. The submitted ring is not modified by the device, so it's
        // safe to read without a volatile operation.
        unsafe { (*self.header.as_ptr()).insertion_counter }
    }

    /// Sets the header's insertion counter.
    ///
    /// All prior writes to the virtqueue's data structures must be made visible
    /// to the device before calling this method.
    fn set_insertion_counter(&mut self, insertion_counter: Wrapping<u16>) {
        // SAFETY: The header pointer is valid while the queue's memory is mapped.
        unsafe {
            std::ptr::write_volatile(
                &raw mut (*self.header.as_ptr()).insertion_counter,
                insertion_counter,
            );
        }
    }
}

/// Typesafe wrapper for [`abi::ReturnedRingEntry`].
pub struct VirtioQueueReturnedBuffer {
    /// See [`abi::ReturnedRingEntry::first_descriptor_index`].
    pub list_head: VirtioQueueDescriptorListHead,

    /// See [`abi::ReturnedRingEntry::written_bytes`].
    pub written_bytes: u32,
}

impl From<&VirtioQueueReturnedBuffer> for VirtioReturnedBufferInfo {
    fn from(returned_buffer: &VirtioQueueReturnedBuffer) -> VirtioReturnedBufferInfo {
        VirtioReturnedBufferInfo {
            submitted_buffer_id: (&returned_buffer.list_head).into(),
            written_bytes: returned_buffer.written_bytes,
        }
    }
}

/// Manages a virtqueue's ring of returned buffers.
///
/// The API and implementation work around the fact that the Rust memory model
/// does not allow creating a reference to any part of the returned ring entry,
/// because the virtio device may write the ring at any time.
///
/// The instance's owner is the exclusive manager of the memory backing all the
/// ring's parts (header, array of ring entries, and trailer). So, at most one
/// instance may exist for a virtqueue.
// @cite(virtio): sec="2.7.8" title="The Virtqueue Used Ring"
pub struct VirtioQueueReturnedRing {
    /// Points to the returned ring's header.
    ///
    /// SAFETY: We assume that the virtio device follows the specification and
    /// populates the returned ring with descriptor indices provided by the
    /// driver in the submitted ring. It follows that each returned ring entry
    /// is a valid descriptor table index that points to the first descriptor of
    /// a submitted buffer.
    header: NonNull<abi::ReturnedRingHeader>,

    /// AND mask that normalizes the ring index.
    ///
    /// The result of `insertion_counter & capacity_mask` is guaranteed to
    /// equal the result of `insertion_counter % capacity` for all valid
    /// values of `insertion_counter`.
    capacity_mask: u16,

    /// Incremented (with overflow) when the driver extracts an entry from the ring.
    extraction_counter: Wrapping<u16>,

    /// This struct owns the ring header's memory.
    _owns_header: PhantomData<abi::ReturnedRingHeader>,

    /// This struct owns the ring entries' memory.
    ///
    /// All ring entries point to valid indices in the associated virtqueue's
    /// descriptor table.
    _owns_entries: PhantomData<[abi::ReturnedRingEntry]>,

    /// This struct owns the ring trailer's memory.
    _owns_trailer: PhantomData<abi::ReturnedRingTrailer>,
}

/// SAFETY: The struct owns the memory that it accesses, so it is conceptually
/// equivalent to any other Rust type that owns its data.
unsafe impl Send for VirtioQueueReturnedRing {}

impl VirtioQueueReturnedRing {
    /// `ring_header` must point to zeroed memory.
    ///
    /// `capacity` is named "Queue Size" in the specification. The value must be a
    /// power of two.
    ///
    /// SAFETY: `ring_header` must point to [`size_bytes()`] bytes that belong
    /// to the same memory allocation. The rest of the driver must not access
    /// the memory range after handing it to this method.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    // @cite(virtio): sec="4.1.5.1.3" title="Virtqueue Configuration"
    // @alias(virtio): theirs="Queue Size" ours="capacity"
    pub unsafe fn new(
        ring_header: NonNull<abi::ReturnedRingHeader>,
        capacity: NonZero<u16>,
    ) -> VirtioQueueReturnedRing {
        debug_assert!((ring_header.as_ptr() as usize) % align_of::<abi::ReturnedRingHeader>() == 0);
        debug_assert!(
            capacity.is_power_of_two(),
            "Virtio split queue capacity is not a power of two: {}",
            capacity,
        );

        VirtioQueueReturnedRing {
            header: ring_header,
            capacity_mask: capacity.get() - 1,
            extraction_counter: Wrapping(0),

            // [`zerocopy::FromBytes`] guarantees that zeroed memory is a valid
            // [`abi::ReturnedRingHeader`] value.
            _owns_header: PhantomData,

            // [`zerocopy::FromBytes`] guarantees that zeroed memory makes up
            // valid [`abi::ReturnedRingEntry`] values. In addition, zero is a valid
            // index for every descriptor table (virtqueue capacity is at least
            // 1).
            _owns_entries: PhantomData,

            // [`zerocopy::FromBytes`] guarantees that zeroed memory is a valid
            // [`abi::ReturnedRingTrailer`] value.
            _owns_trailer: PhantomData,
        }
    }

    /// Attempts to extract a descriptor index from the ring.
    ///
    /// Uses a memory barrier that ensures all previous writes posted to RAM are
    /// seen by the driver.
    // @cite(virtio): sec="2.7.14" title="Receiving Used Buffers From The Device"
    pub fn pop_front(&mut self) -> Option<VirtioQueueReturnedBuffer> {
        // Our implementation is significantly simpler than the reference code for
        // receiving used buffers from the device because we don't disable
        // notifications while reading from the ring. Disabling notification is
        // a performance improvement, not a requirement for correctness.
        // @cite(virtio): sec="2.7.14" title="Receiving Used Buffers From The Device"

        let device_insertion_counter = self.read_insertion_counter();
        if self.extraction_counter == device_insertion_counter {
            return None;
        }

        // This barrier ensures that our driver's memory reads from the
        // virtqueue data structure see the device's writes preceding the
        // counter update.
        atomic::fence(Ordering::Acquire);

        let entry_ptr = self.entry_ptr(self.extraction_counter);

        // SAFETY: `entry_ptr` is guaranteed to be valid by construction.
        let entry = unsafe { std::ptr::read_volatile(entry_ptr.as_ptr()) };

        self.extraction_counter = self.extraction_counter + Wrapping(1);

        debug_assert!(
            entry.first_descriptor_index <= self.capacity_mask as u32,
            "The virtio device returned an invalid descriptor index: {}",
            entry.first_descriptor_index,
        );
        let list_head = VirtioQueueDescriptorListHead {
            first_index: VirtioQueueDescriptorTableIndex::new(entry.first_descriptor_index as u16),
        };

        Some(VirtioQueueReturnedBuffer { list_head, written_bytes: entry.written_bytes })
    }

    /// The number of bytes needed by a ring's data structures.
    // @cite(virtio): sec="2.7" title="Split Virtqueues"
    pub fn size_bytes(capacity: NonZero<u16>) -> NonZero<usize> {
        NonZero::<usize>::new(
            size_of::<abi::ReturnedRingHeader>()
                + size_of::<abi::ReturnedRingEntry>() * (capacity.get() as usize)
                + size_of::<abi::ReturnedRingTrailer>(),
        )
        .expect("Incorrect returned size_bytes computation for the returned ring")
    }

    /// Computes the pointer to a ring entry.
    ///
    /// `counter` must be the ring's insertion counter or extraction counter.
    /// Returns a pointer to the ring entry used by the operation.
    fn entry_ptr(&self, counter: Wrapping<u16>) -> NonNull<abi::ReturnedRingEntry> {
        // SAFETY: All the ring components (header, entries, trailer) belong to
        // the same memory allocation.
        let entries_ptr = unsafe { self.header.as_ptr().add(1) }.cast::<abi::ReturnedRingEntry>();

        // [`Option::unwrap()`] will not panic because `entries_ptr` was computed
        // by adding a [`usize`] to a [`NonNull`] pointer, and the result of the
        // addition is guaranteed to be non-null.
        let entries = NonNull::new(entries_ptr).unwrap();
        debug_assert!((entries.as_ptr() as usize) % align_of::<abi::ReturnedRingEntry>() == 0);

        let entry_index = self.entry_index_from_counter(counter);

        // SAFETY: `entry_index` points to a valid ring entry. All ring entries
        // belong to the same memory allocation.
        unsafe { entries.add(entry_index as usize) }
    }

    /// Computes the ring entry index corresponding to an operation counter.
    ///
    /// `counter` must be the ring's insertion counter or extraction counter.
    /// The returned value is the index of the ring entry used by the operation.
    fn entry_index_from_counter(&self, counter: Wrapping<u16>) -> u16 {
        counter.0 & self.capacity_mask
    }

    /// Reads the insertion counter from the header.
    fn read_insertion_counter(&self) -> Wrapping<u16> {
        // SAFETY: The header pointer is valid while the queue's memory is mapped.
        unsafe { std::ptr::read_volatile(&raw mut (*self.header.as_ptr()).insertion_counter) }
    }
}
