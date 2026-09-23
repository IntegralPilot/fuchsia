// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2016, Google, Inc. All rights reserved
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/lib/pow2_range_allocator/pow2_range_allocator.cc and
// zircon/kernel/lib/pow2_range_allocator/include/lib/pow2_range_allocator.h

#![no_std]

use core::convert::Infallible;
use core::pin::Pin;
use debug::{ltracef, tracef};
use fbl::{Array, DoublyLinkedList, DoublyLinkedListContainable, DoublyLinkedListNode};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;

const LOCAL_TRACE: u32 = 0;

/// Bookkeeping for a single power of 2 sized, power of 2 aligned sub-range.
#[derive(DoublyLinkedListContainable)]
struct Block {
    #[dll_node]
    node: DoublyLinkedListNode<Block>,
    bucket: u32,
    start: u32,
}

impl Block {
    const fn new() -> Self {
        Self { node: DoublyLinkedListNode::new(), bucket: 0, start: 0 }
    }
}

/// Bookkeeping for a range of `u32`s which was handed to the allocator via `add_range`.
#[derive(DoublyLinkedListContainable)]
struct Range {
    #[dll_node]
    node: DoublyLinkedListNode<Range>,
    start: u32,
    len: u32,
}

impl Range {
    const fn new() -> Self {
        Self { node: DoublyLinkedListNode::new(), start: 0, len: 0 }
    }
}

/// Frees every `Block` remaining in `list`.
///
/// The lists hold raw pointers and perform no lifecycle management, so ownership of each element
/// has to be reclaimed explicitly.
fn free_block_list(list: &mut DoublyLinkedList<*mut Block>) {
    while let Some(block) = list.pop_front() {
        // SAFETY: Every `Block` in one of the allocator's lists was allocated with
        // `kalloc::Box::try_new` and leaked with `kalloc::Box::into_raw`.  It has just been removed
        // from `list`, so this is the only pointer to it.
        drop(unsafe { kalloc::Box::from_raw(block) });
    }
}

/// Frees every `Range` remaining in `list`.
fn free_range_list(list: &mut DoublyLinkedList<*mut Range>) {
    while let Some(range) = list.pop_front() {
        // SAFETY: Every `Range` in the allocator's range list was allocated with
        // `kalloc::Box::try_new` and leaked with `kalloc::Box::into_raw`.  It has just been removed
        // from `list`, so this is the only pointer to it.
        drop(unsafe { kalloc::Box::from_raw(range) });
    }
}

/// All of the mutable state of a `Pow2RangeAllocator`, protected by the allocator's mutex.
#[pin_data(PinnedDrop)]
struct Inner {
    #[pin]
    ranges: DoublyLinkedList<*mut Range>,
    #[pin]
    unused_blocks: DoublyLinkedList<*mut Block>,
    #[pin]
    allocated_blocks: DoublyLinkedList<*mut Block>,
    free_block_buckets: Array<DoublyLinkedList<*mut Block>>,
    bucket_count: u32,
}

// SAFETY: The raw pointers held by `Inner`'s lists are exclusively owned by the allocator; they
// point at `kalloc::Box` allocations which are never handed out and which are only ever reachable
// with the allocator's mutex held.  Ownership of the whole state can therefore be moved between
// threads, which is what the C++ implementation does by placing a `Pow2RangeAllocator` in shared
// storage and serializing all access on its `DECLARE_MUTEX`.
unsafe impl Send for Inner {}

#[pinned_drop]
impl PinnedDrop for Inner {
    fn drop(self: Pin<&mut Self>) {
        // The C++ implementation has no destructor at all; a `Pow2RangeAllocator` which is
        // destroyed without calling `Free()` simply leaks its bookkeeping.  Rust's lists hold raw
        // pointers and debug assert that they are empty when dropped, so the bookkeeping is
        // released here instead.
        // SAFETY: We are being dropped, so nothing is moved out of the pinned `Inner`.
        let this = unsafe { self.get_unchecked_mut() };
        this.release();
    }
}

impl Inner {
    fn new() -> impl PinInit<Self, Infallible> {
        pin_init!(Self {
            ranges <- DoublyLinkedList::<*mut Range>::new(),
            unused_blocks <- DoublyLinkedList::<*mut Block>::new(),
            allocated_blocks <- DoublyLinkedList::<*mut Block>::new(),
            free_block_buckets: Array::new(),
            bucket_count: 0,
        })
    }

    fn init(&mut self, bucket_count: u32) -> Result<(), Status> {
        let mut uninit = kalloc::Box::<[DoublyLinkedList<*mut Block>]>::try_new_uninit_slice(
            bucket_count as usize,
        )
        .map_err(|_| {
            tracef!("Failed to allocate storage for {} free bucket lists!\n", bucket_count);
            Status::NO_MEMORY
        })?;
        for item in uninit.iter_mut() {
            let init = DoublyLinkedList::<*mut Block>::new();
            // SAFETY: `item.as_mut_ptr()` is a valid, writable pointer to uninitialized memory
            // in the newly allocated slice.
            unsafe {
                let _ = init.__pinned_init(item.as_mut_ptr());
            }
        }
        // SAFETY: All elements in `uninit` were initialized in the loop above.
        let buf = unsafe { uninit.assume_init() };
        self.free_block_buckets = Array::from_box(buf);
        self.bucket_count = bucket_count;
        Ok(())
    }

    /// Releases every piece of bookkeeping still held by the allocator.
    fn release(&mut self) {
        free_range_list(&mut self.ranges);
        free_block_list(&mut self.unused_blocks);
        free_block_list(&mut self.allocated_blocks);
        for bucket in self.free_block_buckets.iter_mut() {
            free_block_list(bucket);
        }
    }

    /// Returns a block of bookkeeping, recycling one from the unused list if possible.
    ///
    /// Returns `None` if a new block had to be allocated and the allocation failed.
    fn get_unused_block(&mut self) -> Option<*mut Block> {
        if !self.unused_blocks.is_empty() {
            return self.unused_blocks.pop_front();
        }

        match kalloc::Box::try_new(Block::new()) {
            Ok(block) => Some(kalloc::Box::into_raw(block)),
            Err(_) => None,
        }
    }

    /// Returns `block` to its proper free bucket, merging with its buddy as many times as possible.
    ///
    /// # Safety
    ///
    /// `block` must be a valid pointer to a `Block` owned by this allocator which is not currently
    /// a member of any list.
    unsafe fn return_free_block(&mut self, mut block: *mut Block) {
        // The C++ implementation recurses into `ReturnFreeBlock` after a successful merge.  This is
        // the same algorithm written as a loop.
        loop {
            debug_assert!(!block.is_null());
            // SAFETY: The caller guarantees that `block` points at a valid `Block` which is not in
            // any container.  On subsequent loop iterations `block` was just erased from a bucket.
            let block_ref = unsafe { &*block };
            debug_assert!(block_ref.bucket < self.bucket_count);
            debug_assert!(!block_ref.node.in_container());

            let bucket = block_ref.bucket;
            let block_start = block_ref.start;
            let block_len = 1u32 << bucket;
            debug_assert_eq!(block_start & (block_len - 1), 0);

            // Return the block to its proper free bucket, sorted by base ID.  Start by
            // finding the block which should come after this block in the list.
            let list = &mut self.free_block_buckets[bucket as usize];
            let mut inserted = false;
            {
                let mut cursor = list.cursor_front_mut();
                while let Some(after) = cursor.get() {
                    // We do not allow ranges to overlap.
                    let after_len = 1u32 << after.bucket;
                    let after_start = after.start;
                    debug_assert!(
                        (block_start >= after_start.wrapping_add(after_len))
                            || (after_start >= block_start.wrapping_add(block_len))
                    );

                    if after_start > block_start {
                        // SAFETY: `block` is a valid pointer to a `Block` which is not in any
                        // container, and it is owned by this allocator so it outlives its
                        // membership in the list.
                        unsafe { cursor.insert_before_raw(block) };
                        inserted = true;
                        break;
                    }
                    cursor.move_next();
                }
            }

            // If no block comes after this one, it goes on the end of the list.
            if !inserted {
                // SAFETY: See the `insert_before_raw` call above.
                unsafe { list.push_back_raw(block) };
            }

            // After this point, the bucket list owns |block|.

            // Don't merge blocks in the largest bucket.
            if bucket + 1 == self.bucket_count {
                return;
            }

            // Check to see if we should be merging this block into a larger aligned block.
            let (first, second) = if (block_start & ((block_len << 1) - 1)) != 0 {
                // Odd alignment.  This might be the second block of a merge pair.
                // SAFETY: `block` was just inserted into `list`.
                let mut cursor = unsafe { list.cursor_at(&*block) };
                cursor.move_prev();
                let first = cursor.get().map(|b| core::ptr::from_ref(b).cast_mut());
                (first, Some(block))
            } else {
                // Even alignment.  This might be the first block of a merge pair.
                // SAFETY: `block` was just inserted into `list`.
                let mut cursor = unsafe { list.cursor_at(&*block) };
                cursor.move_next();
                let second = cursor.get().map(|b| core::ptr::from_ref(b).cast_mut());
                (Some(block), second)
            };

            // Do these chunks fit together?
            let (Some(first), Some(second)) = (first, second) else {
                return;
            };

            // SAFETY: `first` and `second` are both members of `list` and therefore valid.
            let (first_bucket, first_start, second_bucket, second_start) =
                unsafe { ((*first).bucket, (*first).start, (*second).bucket, (*second).start) };
            let first_len = 1u32 << first_bucket;
            if first_start.wrapping_add(first_len) != second_start {
                return;
            }
            debug_assert_eq!(first_bucket, second_bucket);

            // Remove the two blocks' bookkeeping from their bucket.
            // SAFETY: Both `first` and `second` are valid and are currently members of `list`.
            unsafe {
                let _ = list.erase(&*first);
                let _ = list.erase(&*second);
            }

            // Place one half of the bookkeeping back on the unused list.
            // SAFETY: `second` was just erased from `list`, so it is in no container.
            unsafe { self.unused_blocks.push_back_raw(second) };

            // Reuse the other half to track the newly merged block, and place
            // it in the next bucket size up.
            // SAFETY: `first` was just erased from `list` and is still owned by this allocator.
            unsafe { (*first).bucket += 1 };
            block = first;
        }
    }

    fn add_range(&mut self, mut range_start: u32, mut range_len: u32) -> Result<(), Status> {
        for range in self.ranges.iter() {
            if ((range.start >= range_start) && (range.start < range_start.wrapping_add(range_len)))
                || ((range_start >= range.start)
                    && (range_start < range.start.wrapping_add(range.len)))
            {
                tracef!(
                    "Range [{}, {}] overlaps with existing range [{}, {}].\n",
                    range_start,
                    range_start.wrapping_add(range_len).wrapping_sub(1),
                    range.start,
                    range.start.wrapping_add(range.len).wrapping_sub(1)
                );
                return Err(Status::ALREADY_EXISTS);
            }
        }

        // Allocate our range state.
        let Ok(mut new_range) = kalloc::Box::try_new(Range::new()) else {
            return Err(Status::NO_MEMORY);
        };
        new_range.start = range_start;
        new_range.len = range_len;

        // Break the range we were given into power of two aligned chunks, and place
        // them on the new blocks list to be added to the free-blocks buckets.
        debug_assert!(self.bucket_count != 0);
        debug_assert!(!self.free_block_buckets.is_empty());
        pin_init::stack_pin_init!(let new_blocks = DoublyLinkedList::<*mut Block>::new());
        // SAFETY: `new_blocks` is pinned on the stack and is never moved out of.
        let new_blocks = unsafe { new_blocks.get_unchecked_mut() };

        let mut bucket = self.bucket_count - 1;
        let mut csize = 1u32 << bucket;
        let max_csize = csize;
        while range_len != 0 {
            // Shrink the chunk size until it is aligned with the start of the
            // range, and not larger than the number of irqs we have left.
            let mut shrunk = false;
            while ((range_start & (csize - 1)) != 0) || (range_len < csize) {
                csize >>= 1;
                bucket -= 1;
                shrunk = true;
            }

            // If we didn't need to shrink the chunk size, perhaps we can grow it
            // instead.
            if !shrunk {
                let mut tmp = csize << 1;
                while (tmp <= max_csize) && (tmp <= range_len) && ((range_start & (tmp - 1)) == 0) {
                    bucket += 1;
                    csize = tmp;
                    tmp <<= 1;
                    debug_assert!(bucket < self.bucket_count);
                }
            }

            // Break off a chunk of the range.
            debug_assert_eq!(1u32 << bucket, csize);
            debug_assert!(bucket < self.bucket_count);
            debug_assert_eq!(range_start & (csize - 1), 0);
            debug_assert!(csize <= range_len);
            debug_assert!(csize != 0);

            let Some(block) = self.get_unused_block() else {
                tracef!(
                    "WARNING! Failed to allocate block bookkeeping with sub-range [{}, {}] still left to track.\n",
                    range_start,
                    range_start.wrapping_add(range_len).wrapping_sub(1)
                );
                free_block_list(new_blocks);
                return Err(Status::NO_MEMORY);
            };

            // SAFETY: `block` was just handed to us by `get_unused_block`, so it is valid and is
            // not a member of any list.
            unsafe {
                (*block).bucket = bucket;
                (*block).start = range_start;
                new_blocks.push_back_raw(block);
            }

            range_start += csize;
            range_len -= csize;
        }

        // Looks like we managed to allocate everything we needed to.  Go ahead and
        // add all of our newly allocated bookkeeping to the state.
        // SAFETY: `new_range` was just allocated and is a member of no list.
        unsafe { self.ranges.push_back_raw(kalloc::Box::into_raw(new_range)) };

        while let Some(block) = new_blocks.pop_front() {
            // SAFETY: `block` was just popped from `new_blocks` and is owned by this allocator.
            unsafe { self.return_free_block(block) };
        }

        Ok(())
    }

    fn allocate_range(&mut self, orig_bucket: u32) -> Result<u32, Status> {
        // Find the smallest sized chunk which can hold the allocation and is
        // compatible with the requested addressing capabilities.
        let mut bucket = orig_bucket;
        let mut block: Option<*mut Block> = None;
        while bucket < self.bucket_count {
            block = self.free_block_buckets[bucket as usize].pop_front();
            if block.is_some() {
                break;
            }
            bucket += 1;
        }

        // Nothing found, unlock and get out.
        let Some(block) = block else {
            return Err(Status::NO_RESOURCES);
        };

        // Looks like we have a chunk which can satisfy this allocation request.
        // Split it as many times as needed to match the requested size.
        // SAFETY: `block` was just popped off of a free bucket, so it is valid and in no list.
        debug_assert_eq!(unsafe { (*block).bucket }, bucket);
        debug_assert!(bucket >= orig_bucket);

        while bucket > orig_bucket {
            // If we failed to allocate bookkeeping for the split block, put the block
            // we failed to split back into the free list (merging if required),
            // then fail the allocation.
            let Some(split_block) = self.get_unused_block() else {
                tracef!(
                    "Failed to allocate free bookkeeping block when attempting to split for allocation\n"
                );
                // SAFETY: `block` is valid and is a member of no list.
                unsafe { self.return_free_block(block) };
                return Err(Status::NO_MEMORY);
            };

            debug_assert!(bucket != 0);
            bucket -= 1;

            // SAFETY: Both `block` and `split_block` are valid and are members of no list.
            unsafe {
                // Cut the first chunk in half.
                (*block).bucket = bucket;

                // Fill out the bookkeeping for the second half of the chunk.
                (*split_block).start = (*block).start + (1u32 << (*block).bucket);
                (*split_block).bucket = bucket;

                // Return the second half of the chunk to the free pool.
                self.return_free_block(split_block);
            }
        }

        // Success! Mark the block as allocated and return the block to the user.
        // SAFETY: `block` is valid and is a member of no list.
        let range_start = unsafe { (*block).start };
        // SAFETY: `block` is valid and is a member of no list.
        unsafe { self.allocated_blocks.push_front_raw(block) };

        Ok(range_start)
    }

    fn free_range(&mut self, range_start: u32, bucket: u32) {
        // In a debug build, find the specific block being returned in the list of
        // allocated blocks and use it as the bookkeeping for returning to the free
        // bucket.  Because this is an O(n) operation, and serves only as an integrity
        // check, we only do this in debug builds.  In release builds, we just grab
        // any piece of bookkeeping memory off the allocated_blocks list and use
        // that instead.
        //
        // The C++ implementation selects between the two arms with
        // `#if DEBUG_ASSERT_IMPLEMENTED`.  Rust's `debug_assertions` gates `debug_assert!` in
        // exactly the same way that `DEBUG_ASSERT_IMPLEMENTED` gates `DEBUG_ASSERT`, so it is used
        // here to keep the port self consistent.
        let block: Option<*mut Block> = if cfg!(debug_assertions) {
            self.allocated_blocks.erase_if(|candidate| {
                (candidate.start == range_start) && (candidate.bucket == bucket)
            })
        } else {
            let block = self.allocated_blocks.pop_front();
            if let Some(block) = block {
                // SAFETY: `block` was just popped off of `allocated_blocks`.
                unsafe {
                    (*block).start = range_start;
                    (*block).bucket = bucket;
                }
            }
            block
        };

        let block = block.expect("no matching allocated block to free");

        // Return the block to the free buckets (merging as needed) and we are done.
        // SAFETY: `block` was just removed from `allocated_blocks`, so it is valid and is a member
        // of no list.
        unsafe { self.return_free_block(block) };
    }
}

/// `Pow2RangeAllocator` is a small utility class which partitions a set of
/// ranges of integers into sub-ranges which are power of 2 in length and power
/// of 2 aligned and then manages allocating and freeing the subranges for
/// clients.  It is responsible for breaking larger sub-regions into smaller ones
/// as needed for allocation, and for merging sub-regions into larger sub-regions
/// as needed during free operations.
///
/// Its primary use is as a utility library for platforms who need to manage
/// allocating blocks MSI IRQ IDs on behalf of the PCI bus driver, but could (in
/// theory) be used for other things).
#[ksync::guarded]
pub struct Pow2RangeAllocator {
    #[guarded_by(lock)]
    #[pin]
    inner: Inner,

    #[mutex]
    lock: ksync::KMutex,
}

// Like the C++ implementation, a `Pow2RangeAllocator` is meant to live in shared storage and to
// serialize all of its access on its own mutex.
const _: fn() = || {
    fn assert_sync<T: Sync + ?Sized>() {}
    assert_sync::<Pow2RangeAllocator>();
};

impl Pow2RangeAllocator {
    /// Creates a new, uninitialized `Pow2RangeAllocator`.
    ///
    /// `init` must be called before any range can be added or allocated.
    pub fn new() -> impl PinInit<Self, Infallible> {
        pin_init!(Self {
            inner <- ksync::kcell_init(Inner::new()),
            lock <- ksync::KMutex::init(),
        })
    }

    /// Initialize the state of a pow2 range allocator.
    ///
    /// `max_alloc_size` is the maximum size of a single contiguous allocation.  It must be a power
    /// of 2.
    ///
    /// Returns a status code indicating the success or failure of the operation.
    /// Possible return values include
    /// ++ `ZX_ERR_INVALID_ARGS` `max_alloc_size` is zero or not a power of 2.
    /// ++ `ZX_ERR_NO_MEMORY` Not enough memory to allocate the storage for free bucket lists.
    pub fn init(&self, max_alloc_size: u32) -> Result<(), Status> {
        if (max_alloc_size == 0) || !max_alloc_size.is_power_of_two() {
            tracef!("max_alloc_size ({}) is not an integer power of two!\n", max_alloc_size);
            return Err(Status::INVALID_ARGS);
        }

        let bucket_count = max_alloc_size.ilog2() + 1;

        ksync::lock!(let mut guard = self.lock_lock());
        // SAFETY: `inner` is structurally pinned inside the pinned allocator; taking a `&mut`
        // reference to it never moves it.
        let inner = unsafe { guard.as_mut().inner_mut().get_unchecked_mut() };
        inner.init(bucket_count)
    }

    /// Free all of the state associated with a previously initialized pow2 range allocator.
    pub fn free(&self) {
        ksync::lock!(let mut guard = self.lock_lock());
        // SAFETY: `inner` is structurally pinned inside the pinned allocator; taking a `&mut`
        // reference to it never moves it.
        let inner = unsafe { guard.as_mut().inner_mut().get_unchecked_mut() };

        debug_assert!(inner.bucket_count != 0);
        debug_assert!(!inner.free_block_buckets.is_empty());
        debug_assert!(inner.allocated_blocks.is_empty());

        inner.release();
    }

    /// Add a range of `u32`s to the pool of ranges to be allocated.
    ///
    /// `range_start` is the start of the `u32` range and `range_len` is its length.
    ///
    /// Returns a status code indicating the success or failure of the operation.
    /// Possible return values include
    /// ++ `ZX_ERR_INVALID_ARGS` range_len is zero, or would cause the range to wrap the
    ///    maximum range of a `u32`.
    /// ++ `ZX_ERR_ALREADY_EXISTS` the specified range overlaps with a range already added
    ///    to the allocator.
    /// ++ `ZX_ERR_NO_MEMORY` Not enough memory to allocate the bookkeeping required for
    ///    managing the range.
    pub fn add_range(&self, range_start: u32, range_len: u32) -> Result<(), Status> {
        ltracef!(
            "Adding range [{}, {}]\n",
            range_start,
            range_start.wrapping_add(range_len).wrapping_sub(1)
        );

        if (range_len == 0) || (range_start.wrapping_add(range_len) < range_start) {
            return Err(Status::INVALID_ARGS);
        }

        // Enter the lock and check for overlap with pre-existing ranges.
        ksync::lock!(let mut guard = self.lock_lock());
        // SAFETY: `inner` is structurally pinned inside the pinned allocator; taking a `&mut`
        // reference to it never moves it.
        let inner = unsafe { guard.as_mut().inner_mut().get_unchecked_mut() };
        inner.add_range(range_start, range_len)
    }

    /// Attempt to allocate a range of `u32`s from the available sub-ranges.  The
    /// size of the allocated range must be a power of 2, and if the allocation
    /// succeeds, it is guaranteed to be aligned on a power of 2 boundary matching its
    /// size.
    ///
    /// `size` is the requested size of the region.  On success, the start of the allocated range is
    /// returned.
    ///
    /// Possible error values include
    /// ++ `ZX_ERR_INVALID_ARGS` Multiple reasons, including...
    ///    ++ size is zero.
    ///    ++ size is not a power of two.
    /// ++ `ZX_ERR_NO_RESOURCES` No contiguous, aligned region could be found to satisfy
    ///    the allocation request.
    /// ++ `ZX_ERR_NO_MEMORY` A region could be found, but memory required for bookkeeping
    ///    could not be allocated.
    pub fn allocate_range(&self, size: u32) -> Result<u32, Status> {
        if (size == 0) || !size.is_power_of_two() {
            tracef!("Size ({}) is not an integer power of 2.\n", size);
            return Err(Status::INVALID_ARGS);
        }

        let orig_bucket = size.ilog2();

        // Lock state during allocation.
        ksync::lock!(let mut guard = self.lock_lock());
        // SAFETY: `inner` is structurally pinned inside the pinned allocator; taking a `&mut`
        // reference to it never moves it.
        let inner = unsafe { guard.as_mut().inner_mut().get_unchecked_mut() };

        if orig_bucket >= inner.bucket_count {
            tracef!(
                "Invalid size ({}).  Valid sizes are integer powers of 2 from [1, {}]\n",
                size,
                1u32.checked_shl(inner.bucket_count.wrapping_sub(1)).unwrap_or(0)
            );
            return Err(Status::INVALID_ARGS);
        }

        inner.allocate_range(orig_bucket)
    }

    /// Free a range previously allocated using `allocate_range`.
    ///
    /// `range_start` is the start of the previously allocated range and `size` is its size.
    pub fn free_range(&self, range_start: u32, size: u32) {
        debug_assert!((size != 0) && size.is_power_of_two());

        let bucket = size.ilog2();

        ksync::lock!(let mut guard = self.lock_lock());
        // SAFETY: `inner` is structurally pinned inside the pinned allocator; taking a `&mut`
        // reference to it never moves it.
        let inner = unsafe { guard.as_mut().inner_mut().get_unchecked_mut() };
        inner.free_range(range_start, bucket);
    }
}
