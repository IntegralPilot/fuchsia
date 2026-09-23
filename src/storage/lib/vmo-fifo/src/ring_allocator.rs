// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A token representing an uncommitted allocation block in the VMO.
#[derive(Debug, PartialEq, Eq)]
pub struct AllocationToken {
    /// The physical byte offset within the payload region where the slice should be copied.
    offset: u32,

    /// Bytes (including alignment padding) consumed by this token. This is used to cancel
    /// allocations (e.g. if the corresponding message is not successfully queued).
    bytes_added: u64,
}

impl AllocationToken {
    /// Gets the physical byte offset within the payload region where the data is stored.
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// Gets the bytes (including alignment padding) consumed by this token.
    pub(crate) fn bytes_added(&self) -> u64 {
        self.bytes_added
    }
}

// A ring buffer allocator intended to be used on the payload region of `SharedQueue`.
pub(crate) struct RingAllocator {
    // The size of the shared VMO payload region in bytes.
    payload_capacity: u64,

    // The maximum number of message slots in the shared VMO queue.
    queue_capacity: u64,

    // The byte alignment boundaries required for all allocations (e.g., 4096).
    // Must be a power of two: The byte padding calculations use bitwise masking `(x + (a - 1)) &
    // !(a - 1)`. This avoids costly division instructions but fundamentally limits alignment bounds
    // to base-2 (powers of two).
    alignment: u64,

    // The cumulative number of bytes historically allocated.
    allocated_bytes: u64,

    // The cumulative number of bytes successfully freed from the queue.
    freed_bytes: u64,

    // Tracks the last read index evaluated for reclamation so we don't scan slots twice.
    last_reclaimed_read_index: u64,

    // Stores a snapshot of `allocated_bytes` for each slot index. When slots are reclaimed, this is
    // read to release the unused allocations and advance `freed_bytes` to the reclaim target.
    reclaim_targets: Box<[Option<u64>]>,
}

impl RingAllocator {
    // Creates a new `RingAllocator`.
    //
    // - `payload_capacity`: The total size of the payload region in bytes.
    // - `queue_capacity`: The maximum number of message slots in the SharedQueue.
    // - `alignment`: The byte alignment boundaries required for allocations. Modulo padding
    //                is applied so that all payload offsets begin aligned to this size. Must be a
    //                power of two.
    pub(crate) fn new(payload_capacity: usize, queue_capacity: usize, alignment: usize) -> Self {
        assert!(
            alignment > 0 && alignment.is_power_of_two(),
            "Alignment must be a power of two to support bitwise alignment arithmetic."
        );
        Self {
            payload_capacity: payload_capacity as u64,
            queue_capacity: queue_capacity as u64,
            alignment: alignment as u64,
            allocated_bytes: 0,
            freed_bytes: 0,
            last_reclaimed_read_index: 0,
            reclaim_targets: vec![None; queue_capacity].into_boxed_slice(),
        }
    }

    // Attempts to allocate `size` bytes of strictly contiguous space in the VMO payload region.
    // Returns an AllocationToken on success, or None if there is not enough space.
    pub(crate) fn allocate(&mut self, size: usize) -> Option<AllocationToken> {
        let align = self.alignment;

        // Enforce size alignment: round it up to the nearest multiple of alignment.
        let aligned_size = (size as u64 + align - 1) & !(align - 1);
        if aligned_size > self.payload_capacity {
            return None;
        }

        let physical_head = self.allocated_bytes % self.payload_capacity;
        // Padding required to align the physical_head offset
        let padding = (align - (physical_head % align)) % align;
        let padded_size = aligned_size + padding;

        let space_to_end = self.payload_capacity - physical_head;
        let (bytes_added, offset) = if padded_size <= space_to_end {
            // It physically fits without wrapping around.
            (padded_size, (physical_head + padding) as u32)
        } else {
            // It does not fit before the end of the VMO, abandon the remaining end of the VMO and
            // wrap around. The new physical offset is 0 (which is inherently aligned).
            (space_to_end + aligned_size, 0)
        };

        let active_bytes = self.allocated_bytes - self.freed_bytes;
        if active_bytes + bytes_added <= self.payload_capacity {
            self.allocated_bytes += bytes_added;
            Some(AllocationToken { offset, bytes_added })
        } else {
            None
        }
    }

    // Informs the allocator that the most recent allocation has been successfully queued. Link this
    // allocation to a slot index for later reclamation.
    pub(crate) fn commit_allocation_to_slot(&mut self, slot_index: u64, _token: AllocationToken) {
        let index = (slot_index % self.queue_capacity) as usize;
        self.reclaim_targets[index] = Some(self.allocated_bytes);
    }

    // Cancel the uncommitted allocation.
    pub(crate) fn cancel_allocation(&mut self, token: AllocationToken) {
        self.allocated_bytes -= token.bytes_added();
    }

    // Frees memory associated with messages that the receiver has finished processing.
    // TODO(https://fxbug.dev/530494057): Add function to decommit memory pages from the VMO when
    // they are reclaimed to avoid holding onto physical RAM while the device is idle.
    pub(crate) fn reclaim_consumed_slots(&mut self, new_read_index: u64) {
        for i in self.last_reclaimed_read_index..new_read_index {
            let slot = (i % self.queue_capacity) as usize;

            if let Some(target) = self.reclaim_targets[slot].take() {
                if target > self.freed_bytes {
                    self.freed_bytes = target;
                }
            }
        }
        self.last_reclaimed_read_index = new_read_index;

        // If the receiver has consumed all outstanding messages, the logical buffer is empty.
        // Reset both pointers back to 0 to promote reuse of memory already in the cache.
        if self.allocated_bytes == self.freed_bytes {
            self.allocated_bytes = 0;
            self.freed_bytes = 0;
        }
    }

    // Returns false if the requested size is permanently impossible to fit inside the payload
    // region, regardless of how much space the queue eventually frees up.
    pub(crate) fn is_within_capacity(&self, size: usize) -> bool {
        let align = self.alignment;
        let aligned_size = (size as u64 + align - 1) & !(align - 1);
        aligned_size <= self.payload_capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_bytes(allocator: &RingAllocator) -> u64 {
        allocator.allocated_bytes - allocator.freed_bytes
    }

    #[test]
    fn test_allocate_sequential() {
        let mut allocator = RingAllocator::new(100, 10, 8);

        let t1 = allocator.allocate(20).unwrap();
        assert_eq!(t1.offset(), 0);
        assert_eq!(t1.bytes_added(), 24); // rounded up to 24 by 8 byte align
        allocator.commit_allocation_to_slot(0, t1);

        let t2 = allocator.allocate(32).unwrap();
        assert_eq!(t2.offset(), 24);
        assert_eq!(t2.bytes_added(), 32);
        allocator.commit_allocation_to_slot(1, t2);

        assert_eq!(active_bytes(&allocator), 56);
    }

    #[test]
    fn test_cancel_allocation() {
        let mut allocator = RingAllocator::new(100, 10, 8);

        let t1 = allocator.allocate(50).unwrap();
        assert_eq!(active_bytes(&allocator), 56);

        allocator.cancel_allocation(t1);

        // Active bytes perfectly zeroed out! Ready to allocate anew.
        assert_eq!(active_bytes(&allocator), 0);
        assert_eq!(allocator.allocated_bytes, 0);
    }

    #[test]
    fn test_empty_vs_full() {
        let mut allocator = RingAllocator::new(128, 10, 64);

        let t1 = allocator.allocate(64).unwrap();
        assert_eq!(active_bytes(&allocator), 64);
        allocator.commit_allocation_to_slot(0, t1);

        let t2 = allocator.allocate(64).unwrap();
        assert_eq!(active_bytes(&allocator), 128);
        allocator.commit_allocation_to_slot(1, t2);

        // Allocate should return None as ring is full.
        assert!(allocator.allocate(10).is_none());

        allocator.reclaim_consumed_slots(1);
        assert_eq!(active_bytes(&allocator), 64);

        allocator.reclaim_consumed_slots(2);
        assert_eq!(active_bytes(&allocator), 0);
    }
}
