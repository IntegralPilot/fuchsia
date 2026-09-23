// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # RegionAllocator
//!
//! ## Overview
//! A `RegionAllocator` is a utility class designed to help with the bookkeeping
//! involved in managing the allocation/partitioning of a 64-bit space into
//! non-overlapping "Regions". In addition to the `RegionAllocator`, there are two
//! other classes involved in the use of a `RegionAllocator`;
//! `Region` and `RegionPool`.
//!
//! A `Region` consists of an unsigned 64-bit base address and an unsigned 64-bit
//! size. A `Region` is considered valid iff its size is non-zero, and it does not
//! wrap its 64-bit space.
//!
//! See the "Memory Allocation" section for a discussion of the `RegionPool`.
//!
//! `RegionAllocator` users can create an allocator and then add any number of
//! non-overlapping Regions to its pool of regions available for allocation.
//! They may then request that regions be allocated from the pool either by
//! requesting that a region be allocated with a particular size/alignment, or
//! by asking for a specific base/size. The `RegionAllocator` will manage all of
//! the bookkeeping involved in breaking available regions into smaller chunks,
//! tracking allocated regions, and re-merging regions when they are returned to
//! the allocator.
//!
//! ## Memory Allocation
//! `RegionAllocator`s require dynamically allocated memory in order to store the
//! bookkeeping required for managing available regions. In order to control
//! heap fragmentation and the frequency of heap interaction, a `RegionPool` object
//! may be used to allocate bookkeeping overhead in larger slabs which are carved up
//! and placed on a free list to be used by a `RegionAllocator`. `RegionPool`s are
//! created with a defined slab size as well as a maximum memory limit. The pool
//! will initially allocate a single slab, but will attempt to grow any time
//! bookkeeping is needed but the free list is empty and the allocation of
//! another slab would not push the allocator over its maximum memory limit.
//!
//! `RegionPool`s are ref-counted objects (`RefPtr<RegionPool>`) that may be shared by multiple
//! `RegionAllocator`s. This allows sub-systems which use multiple allocators to
//! impose system-wide limits on bookkeeping overhead. If a `RegionPool` allocator
//! is to be used, it must be assigned to the `RegionAllocator` before any regions
//! can be added or allocated, and the pool may not be re-assigned while the
//! allocator is using any bookkeeping from the pool.
//!
//! ## APIs and Object lifecycle management
//! The API makes use of `fbl` managed pointer types in order to simplify lifecycle
//! management. `RegionPool`s are managed with `RefPtr<RegionPool>` while `Region`s are handed
//! out via `UniquePtr<Region>`. `RegionAllocator`s themselves impose no lifecycle
//! restrictions and may be heap allocated, stack allocated, or embedded directly
//! in objects as the user sees fit. It is an error to allow a `RegionAllocator`
//! to destruct while there are allocations in flight.
//!
//! ## Thread Safety
//! `RegionAllocator` and `RegionPool`s use `KMutex` or `RawMutex` objects to provide thread
//! safety in multi-threaded environments. As such, `RegionAllocator`s are not
//! currently suitable for use in code which may run at IRQ context, or which
//! must never block.
//!
//! Each `RegionAllocator` has its own mutex allowing for concurrent access across
//! multiple allocators, even when the allocators share the same `RegionPool`.
//! `RegionPool`s also hold their own mutex which may be obtained by an Allocator
//! while holding the Allocator's Mutex.
//!
//! ## Simple Usage Example
//!
//! ```rust
//! use pin_init::stack_pin_init;
//! use region_alloc::{RegionAllocator, RegionPool, RegionSpan, AllowOverlap};
//! use zx_status::Status;
//!
//! # fn main() -> Result<(), Status> {
//! // Create a pool and assign it to a stack allocated allocator. Limit the
//! // bookkeeping memory to 32KB. This will ensure that no heap interactions
//! // take place after startup (during operation).
//! let pool = RegionPool::create(32 << 10).map_err(|_| Status::NO_MEMORY)?;
//! stack_pin_init!(let alloc = RegionAllocator::init_with_pool(pool));
//!
//! // Add regions to the pool which can be allocated from
//! // [3GB,   4GB)
//! alloc.add_region(RegionSpan { base: 0xC000_0000, size: 0x4000_0000 }, AllowOverlap::No)?;
//! // [256GB, 257GB)
//! alloc.add_region(RegionSpan { base: 0x40_0000_0000, size: 0x4000_0000 }, AllowOverlap::No)?;
//!
//! // Grab some specific regions out of the available regions.
//! // [3GB + 1MB,   3GB + 2MB)
//! let r1 = alloc.get_region_specific(RegionSpan { base: 0xC010_0000, size: 0x10_0000 })?;
//! // [256GB + 1MB, 256GB + 2MB)
//! let r2 = alloc.get_region_specific(RegionSpan { base: 0x40_0010_0000, size: 0x10_0000 })?;
//!
//! // Grab some pointer aligned regions of various sizes
//! let r3 = alloc.get_region_pointer_aligned(1024)?;
//! let r4 = alloc.get_region_pointer_aligned(75)?;
//! let r5 = alloc.get_region_pointer_aligned(80000)?;
//!
//! // Grab some page aligned regions of various sizes
//! let r6 = alloc.get_region(1024,  4 << 10)?;
//! let r7 = alloc.get_region(75,    4 << 10)?;
//! let r8 = alloc.get_region(80000, 4 << 10)?;
//!
//! // Access base and size:
//! assert_eq!(r3.size(), 1024);
//! assert_eq!(r8.size(), 80000);
//!
//! // No need to clean up. Regions will automatically be returned to the
//! // allocator as they go out of scope. Then the allocator will return all of
//! // its available regions to the pool when it goes out of scope. Finally, the
//! // pool will free all of its memory as the allocator releases its reference
//! // to the pool.
//! # Ok(())
//! # }
//! ```

#![no_std]

use core::ptr::NonNull;
use fbl::{
    Recyclable, RefPtr, TrackingSize, UniquePtr, WavlTree, WavlTreeContainable, WavlTreeKeyable,
    WavlTreeNode,
};
use kalloc::{AllocError, Box};
use ksync::{KMutex, RawMutex, guarded, kcell_init, lock};
use zx_status::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionSpan {
    pub base: u64,
    pub size: u64,
}

impl RegionSpan {
    pub fn end(&self) -> Result<u64, Status> {
        self.base.checked_add(self.size).ok_or(Status::INVALID_ARGS)
    }

    pub fn validate(&self) -> Result<(), Status> {
        if self.size == 0 {
            return Err(Status::INVALID_ARGS);
        }
        let _ = self.end()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionKey {
    pub base: u64,
    pub size: u64,
}

impl Ord for RegionKey {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        match self.size.cmp(&other.size) {
            core::cmp::Ordering::Equal => self.base.cmp(&other.base),
            ord => ord,
        }
    }
}

impl PartialOrd for RegionKey {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub struct SortByBase;
pub struct SortBySize;

#[derive(WavlTreeContainable)]
pub struct Region {
    span: RegionSpan,
    key_size: RegionKey,
    owner: NonNull<RegionAllocator>,

    #[wavl_node(tag = SortByBase)]
    node_base: WavlTreeNode<Region>,

    #[wavl_node(tag = SortBySize)]
    node_size: WavlTreeNode<Region>,
}

// SAFETY: Region doesn't hold thread-local data and can be safely sent across thread boundaries.
unsafe impl Send for Region {}
// SAFETY: Region's fields are only mutated under the allocator's lock, so it's Sync.
unsafe impl Sync for Region {}

// SAFETY: The Recyclable trait requires that `recycle` is safe to call with a valid pointer.
// `Region` handles recycling via its allocator owner.
unsafe impl Recyclable for Region {
    /// Recycle a region when its `UniquePtr` goes out of scope.
    ///
    /// # Safety
    /// The caller must guarantee that `ptr` is a valid, unique pointer to a `Region`
    /// that is no longer referenced anywhere else.
    unsafe fn recycle(ptr: NonNull<Self>) {
        // SAFETY: `ptr` is verified to be valid and dereferenceable.
        // We release the region back to its owner.
        // `owner` is guaranteed to outlive all regions in flight.
        unsafe {
            let owner = ptr.as_ref().owner;
            owner.as_ref().release_region(ptr);
        }
    }
}

impl Region {
    pub fn base(&self) -> u64 {
        self.span.base
    }
    pub fn size(&self) -> u64 {
        self.span.size
    }

    fn new(span: RegionSpan, owner: NonNull<RegionAllocator>) -> Self {
        Self {
            span,
            key_size: RegionKey { base: span.base, size: span.size },
            owner,
            node_base: WavlTreeNode::new(),
            node_size: WavlTreeNode::new(),
        }
    }

    fn update_key(&mut self) {
        self.key_size = RegionKey { base: self.span.base, size: self.span.size };
    }
}

impl WavlTreeKeyable<u64> for Region {
    type Key<'a> = u64;
    fn get_key(&self) -> u64 {
        self.span.base
    }
}

impl WavlTreeKeyable<RegionKey> for Region {
    type Key<'a> = RegionKey;
    fn get_key(&self) -> RegionKey {
        self.key_size
    }
}

#[fbl::ref_counted]
#[pin_init::pin_data]
#[derive(fbl::Recyclable)]
#[repr(C)]
pub struct RegionPool {
    #[pin]
    allocator: fbl::SlabAllocator<Region, RawMutex, { RegionAllocator::REGION_POOL_SLAB_SIZE }>,
}

impl RegionPool {
    pub fn create(max_memory: usize) -> Result<RefPtr<Self>, AllocError> {
        let slab_size = RegionAllocator::REGION_POOL_SLAB_SIZE;
        if slab_size > max_memory {
            return Err(AllocError);
        }
        let max_slabs = max_memory / slab_size;

        let pool = fbl::pin_make_ref_counted!(Self {
            allocator <- fbl::SlabAllocator::init(max_slabs),
        })
        .map_err(|_| AllocError)?;

        pool.allocator.preallocate()?;
        Ok(pool)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowOverlap {
    No,
    Yes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowIncomplete {
    No,
    Yes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRegionSet {
    Allocated,
    Available,
}

#[guarded]
#[pin_init::pin_data(PinnedDrop)]
pub struct RegionAllocator {
    #[mutex]
    mu: KMutex,

    #[pin]
    // TODO(https://fxbug.dev/536051491): Remove this once WavlTree is pin-aware.
    #[ksync(unpinned)]
    #[guarded_by(mu)]
    allocated_regions_by_base: WavlTree<u64, NonNull<Region>, SortByBase, TrackingSize>,

    #[pin]
    // TODO(https://fxbug.dev/536051491): Remove this once WavlTree is pin-aware.
    #[ksync(unpinned)]
    #[guarded_by(mu)]
    avail_regions_by_base: WavlTree<u64, NonNull<Region>, SortByBase, TrackingSize>,

    #[pin]
    // TODO(https://fxbug.dev/536051491): Remove this once WavlTree is pin-aware.
    #[ksync(unpinned)]
    #[guarded_by(mu)]
    avail_regions_by_size: WavlTree<RegionKey, NonNull<Region>, SortBySize, TrackingSize>,

    #[guarded_by(mu)]
    region_pool: Option<RefPtr<RegionPool>>,
}

// SAFETY: RegionAllocator uses a mutex (`mu`) to synchronize access to its internal state,
// making it safe to send across threads.
unsafe impl Send for RegionAllocator {}
// SAFETY: RegionAllocator uses a mutex (`mu`) to synchronize access to its internal state,
// making it safe to share across threads.
unsafe impl Sync for RegionAllocator {}

impl RegionAllocator {
    pub const REGION_POOL_SLAB_SIZE: usize = 4096;

    pub fn init() -> impl pin_init::PinInit<Self, core::convert::Infallible> {
        pin_init::pin_init!(Self {
            mu <- KMutex::init(),
            allocated_regions_by_base <- kcell_init(WavlTree::new()),
            avail_regions_by_base <- kcell_init(WavlTree::new()),
            avail_regions_by_size <- kcell_init(WavlTree::new()),
            region_pool: None.into(),
        })
    }

    /// Initialize a `RegionAllocator` with a specified `RegionPool`.
    pub fn init_with_pool(
        pool: RefPtr<RegionPool>,
    ) -> impl pin_init::PinInit<Self, core::convert::Infallible> {
        pin_init::pin_init!(Self {
            mu <- KMutex::init(),
            allocated_regions_by_base <- kcell_init(WavlTree::new()),
            avail_regions_by_base <- kcell_init(WavlTree::new()),
            avail_regions_by_size <- kcell_init(WavlTree::new()),
            region_pool: Some(pool).into(),
        })
    }

    pub fn has_region_pool(&self) -> bool {
        lock!(let guard = self.lock_mu());
        guard.fields().region_pool.is_some()
    }

    pub fn set_region_pool(&self, pool: RefPtr<RegionPool>) -> Result<(), Status> {
        lock!(let mut guard = self.lock_mu());
        let fields = guard.as_mut().fields_mut();

        if !fields.allocated_regions_by_base.is_empty() || !fields.avail_regions_by_base.is_empty()
        {
            return Err(Status::BAD_STATE);
        }

        *fields.region_pool = Some(pool);
        Ok(())
    }

    pub fn reset(&self) {
        lock!(let mut guard = self.lock_mu());
        let fields = guard.as_mut().fields_mut();

        debug_assert!(fields.allocated_regions_by_base.is_empty());

        fields.avail_regions_by_base.clear();
        while let Some(region_ptr) = fields.avail_regions_by_size.pop_front() {
            // SAFETY: The popped `region_ptr` is a valid pointer to a `Region` that was stored in
            // the allocator's available trees.  It has been removed from `avail_regions_by_size`,
            // and `avail_regions_by_base` was cleared, so there are no other references to it.
            unsafe {
                Self::destroy_region_raw(fields.region_pool, region_ptr);
            }
        }
    }

    pub fn add_region(
        &self,
        region: RegionSpan,
        allow_overlap: AllowOverlap,
    ) -> Result<(), Status> {
        region.validate()?;

        lock!(let mut guard = self.lock_mu());
        let mut fields = guard.as_mut().fields_mut();

        self.add_subtract_sanity_check_locked_mut(fields.allocated_regions_by_base, &region)?;

        if allow_overlap != AllowOverlap::Yes {
            let intersects = self.intersects_locked(fields.avail_regions_by_base, &region)?;
            if intersects {
                return Err(Status::INVALID_ARGS);
            }
        }

        let region_ptr = self.create_region_raw(fields.region_pool, region)?;

        self.add_region_to_avail_locked(&mut fields, region_ptr, allow_overlap);
        Ok(())
    }

    pub fn subtract_region(
        &self,
        to_subtract: RegionSpan,
        allow_incomplete: AllowIncomplete,
    ) -> Result<(), Status> {
        to_subtract.validate()?;
        let region_end = to_subtract.end()?;

        lock!(let mut guard = self.lock_mu());
        let fields = guard.as_mut().fields_mut();

        self.add_subtract_sanity_check_locked_mut(fields.allocated_regions_by_base, &to_subtract)?;

        let mut region = to_subtract;

        let mut before_contains = false;
        let mut before_ptr = None;
        let mut before_end = 0;

        {
            let mut before_cursor = fields.avail_regions_by_base.upper_bound(&region.base);
            before_cursor.move_prev();

            if let Some(before) = before_cursor.get() {
                before_end = before.base() + before.size();
                if region.base >= before.base() && region_end <= before_end {
                    before_contains = true;
                    before_ptr = Some(NonNull::from(before));
                }
            }

            if before_contains {
                // SAFETY: `before_ptr` is confirmed to be `Some` containing a valid, non-null
                // pointer to a `Region` in the available set.
                let before = unsafe { before_ptr.unwrap().as_ref() };

                // Case 1: Same region
                if region.base == before.base() && region_end == before_end {
                    let removed_ptr = before_cursor.erase().unwrap();
                    let key = before.key_size;
                    fields.avail_regions_by_size.erase(&key);
                    // SAFETY: `removed_ptr` has been removed from all indices and can be safely
                    // destroyed.
                    unsafe {
                        Self::destroy_region_raw(fields.region_pool, removed_ptr);
                    }
                    return Ok(());
                }

                // Case 2: Split in middle
                if region.base != before.base() && region_end != before_end {
                    // The allocator lock is held. We are creating a new region to hold the second
                    // half of the split.
                    let second_ptr = self.create_region_raw(
                        fields.region_pool,
                        RegionSpan { base: region_end, size: before_end - region_end },
                    )?;
                    let key = before.key_size;
                    let first_ptr = fields.avail_regions_by_size.erase(&key).unwrap();

                    // SAFETY: `first_ptr` is a valid pointer to a `Region`. Exclusivity is
                    // guaranteed because we hold the allocator lock, and although the region
                    // remains in `avail_regions_by_base` (its base address is unchanged), we have
                    // erased it from `avail_regions_by_size` and will not access it through the
                    // base tree during mutation.
                    unsafe {
                        let first = &mut *first_ptr.as_ptr();

                        first.span.size = region.base - first.base();
                        first.update_key();
                    }

                    // SAFETY: We insert the modified `first_ptr` and newly created `second_ptr`
                    // back into the WavlTree indices.  The trees will now own these pointers.
                    unsafe {
                        fields.avail_regions_by_size.insert_raw(first_ptr);
                        fields.avail_regions_by_base.insert_raw(second_ptr);
                        fields.avail_regions_by_size.insert_raw(second_ptr);
                    }
                    return Ok(());
                }

                // Case 3: Trim front
                if region.base == before.base() {
                    let key = before.key_size;
                    let bptr = fields.avail_regions_by_size.erase(&key).unwrap();
                    // SAFETY: `bptr` is a valid pointer to a `Region`.
                    let base = unsafe { bptr.as_ref().base() };
                    fields.avail_regions_by_base.erase(&base);

                    // SAFETY: `bptr` is a valid pointer to a `Region`. Exclusivity is guaranteed
                    // because we have erased the region from both the size and base available trees
                    // (since its base address is changing).
                    unsafe {
                        let b = &mut *bptr.as_ptr();
                        b.span.base += region.size;
                        b.span.size -= region.size;
                        b.update_key();
                    }

                    // SAFETY: Re-inserting the trimmed region pointer back into the available
                    // indices is safe.
                    unsafe {
                        fields.avail_regions_by_size.insert_raw(bptr);
                        fields.avail_regions_by_base.insert_raw(bptr);
                    }
                    return Ok(());
                }

                // Case 4: Trim end
                let key = before.key_size;
                let bptr = fields.avail_regions_by_size.erase(&key).unwrap();
                // SAFETY: `bptr` is a valid pointer to a `Region`. Exclusivity is guaranteed
                // because we hold the allocator lock, and although the region remains in
                // `avail_regions_by_base` (its base address is unchanged), we have erased it from
                // `avail_regions_by_size` and will not access it through the base tree during
                // mutation.
                unsafe {
                    let b = &mut *bptr.as_ptr();
                    b.span.size -= region.size;
                    b.update_key();
                }
                // SAFETY: Re-inserting the trimmed region pointer back into the available index is
                // safe.
                unsafe {
                    fields.avail_regions_by_size.insert_raw(bptr);
                }
                return Ok(());
            }
        } // before_cursor dropped

        if allow_incomplete != AllowIncomplete::Yes {
            return Err(Status::INVALID_ARGS);
        }

        {
            let mut before_cursor = fields.avail_regions_by_base.upper_bound(&region.base);
            before_cursor.move_prev();
            if before_cursor.get().is_some() {
                let before = before_cursor.get().unwrap();
                let before_end = before.base() + before.size();
                if before_end > region.base {
                    if before.base() == region.base {
                        let removed_ptr = before_cursor.erase().unwrap();
                        // SAFETY: `removed_ptr` is a valid pointer.
                        let key = unsafe { removed_ptr.as_ref().key_size };
                        fields.avail_regions_by_size.erase(&key);
                        // SAFETY: `removed_ptr` has been removed from all indices and can be safely
                        // destroyed.
                        unsafe {
                            Self::destroy_region_raw(fields.region_pool, removed_ptr);
                        }
                    } else {
                        let key = before.key_size;
                        let bptr = fields.avail_regions_by_size.erase(&key).unwrap();
                        // SAFETY: `bptr` is a valid pointer to a `Region`. Exclusivity is
                        // guaranteed because we hold the allocator lock, and although the region
                        // remains in `avail_regions_by_base` (its base address is unchanged), we
                        // have erased it from `avail_regions_by_size` and will not access it
                        // through the base tree during mutation.
                        unsafe {
                            let b = &mut *bptr.as_ptr();
                            b.span.size = region.base - b.base();
                            b.update_key();
                        }
                        // SAFETY: Re-inserting the trimmed region pointer back into the available
                        // index is safe.
                        unsafe {
                            fields.avail_regions_by_size.insert_raw(bptr);
                        }
                    }
                    region.base = before_end;
                    region.size = region_end - region.base;
                }
            }
        } // before_cursor dropped

        let mut after_cursor = fields.avail_regions_by_base.upper_bound(&region.base);
        while after_cursor.get().is_some() {
            let after = after_cursor.get().unwrap();
            if after.base() >= region_end {
                break;
            }

            let after_end = after.base() + after.size();

            if after_end > region_end {
                // Trim front
                let trim_ptr = after_cursor.erase().unwrap();
                // SAFETY: `trim_ptr` is a valid pointer.
                let key = unsafe { trim_ptr.as_ref().key_size };
                fields.avail_regions_by_size.erase(&key);

                // SAFETY: `trim_ptr` is a valid pointer to a `Region`. Exclusivity is guaranteed
                // because we have erased the region from both the size and base available trees
                // (since its base address is changing).
                unsafe {
                    let t = &mut *trim_ptr.as_ptr();
                    t.span.base = region_end;
                    t.span.size = after_end - t.span.base;
                    t.update_key();
                }
                // SAFETY: Re-inserting the trimmed region pointer back into the available indices
                // is safe.
                unsafe {
                    fields.avail_regions_by_size.insert_raw(trim_ptr);
                    fields.avail_regions_by_base.insert_raw(trim_ptr);
                }
                break;
            }

            let trim_ptr = after_cursor.erase().unwrap();
            // SAFETY: `trim_ptr` is a valid pointer.
            let key = unsafe { trim_ptr.as_ref().key_size };
            fields.avail_regions_by_size.erase(&key);

            region.base = after_end;
            region.size = region_end - region.base;
            // SAFETY: `trim_ptr` has been removed from all indices and can be safely destroyed.
            unsafe {
                Self::destroy_region_raw(fields.region_pool, trim_ptr);
            }

            if region.size == 0 {
                break;
            }
        }

        debug_assert_eq!(fields.avail_regions_by_base.len(), fields.avail_regions_by_size.len());
        Ok(())
    }

    pub fn get_region(&self, size: u64, alignment: u64) -> Result<UniquePtr<Region>, Status> {
        if size == 0 || alignment == 0 || !alignment.is_power_of_two() {
            return Err(Status::INVALID_ARGS);
        }

        lock!(let mut guard = self.lock_mu());
        let mut fields = guard.as_mut().fields_mut();

        let mask = alignment - 1;
        let inv_mask = !mask;

        let search_key = RegionKey { base: 0, size };
        let mut iter = fields.avail_regions_by_size.lower_bound(&search_key);

        let mut aligned_base = 0;
        let mut found_key = None;

        while iter.get().is_some() {
            let r = iter.get().unwrap();
            debug_assert!(r.size() >= size);

            // Align base
            aligned_base = (r.base() + mask) & inv_mask;
            let overhead = aligned_base - r.base();
            let leftover = r.size() - size;

            if aligned_base >= r.base() && overhead <= leftover {
                found_key = Some(r.key_size);
                break;
            }
            iter.move_next();
        }

        if found_key.is_none() {
            return Err(Status::NOT_FOUND);
        }

        // iter is dropped here
        self.alloc_from_avail_locked(&mut fields, found_key.unwrap(), aligned_base, size)
    }

    /// Get a region out of the set of currently available regions which has a
    /// specified size and is pointer-aligned (aligned to `core::mem::size_of::<*const ()>()`).
    pub fn get_region_pointer_aligned(&self, size: u64) -> Result<UniquePtr<Region>, Status> {
        self.get_region(size, core::mem::size_of::<*const ()>() as u64)
    }

    pub fn get_region_specific(
        &self,
        requested_region: RegionSpan,
    ) -> Result<UniquePtr<Region>, Status> {
        requested_region.validate()?;
        let base = requested_region.base;
        let size = requested_region.size;

        lock!(let mut guard = self.lock_mu());
        let mut fields = guard.as_mut().fields_mut();

        let mut iter = fields.avail_regions_by_base.upper_bound(&base);
        iter.move_prev();

        if !iter.get().is_some() {
            return Err(Status::NOT_FOUND);
        }

        let r = iter.get().unwrap();
        debug_assert!(r.size() > 0);
        debug_assert!(r.base() <= base);

        let req_end = base + size - 1;
        let iter_end = r.base() + r.size() - 1;
        if req_end > iter_end {
            return Err(Status::NOT_FOUND);
        }

        let source_key = r.key_size;
        // iter is dropped here

        self.alloc_from_avail_locked(&mut fields, source_key, base, size)
    }

    pub fn test_region_intersects(
        &self,
        region: RegionSpan,
        which: TestRegionSet,
    ) -> Result<bool, Status> {
        lock!(let mut guard = self.lock_mu());
        let fields = guard.as_mut().fields_mut();
        let tree = match which {
            TestRegionSet::Allocated => fields.allocated_regions_by_base,
            TestRegionSet::Available => fields.avail_regions_by_base,
        };
        self.intersects_locked(tree, &region)
    }

    pub fn test_region_contained_by(
        &self,
        region: RegionSpan,
        which: TestRegionSet,
    ) -> Result<bool, Status> {
        lock!(let mut guard = self.lock_mu());
        let fields = guard.as_mut().fields_mut();
        let tree = match which {
            TestRegionSet::Allocated => fields.allocated_regions_by_base,
            TestRegionSet::Available => fields.avail_regions_by_base,
        };
        self.contained_by_locked(tree, &region)
    }

    pub fn allocated_region_count(&self) -> usize {
        lock!(let guard = self.lock_mu());
        guard.fields().allocated_regions_by_base.len()
    }

    pub fn available_region_count(&self) -> usize {
        lock!(let guard = self.lock_mu());
        guard.fields().avail_regions_by_base.len()
    }

    /// Walk the allocated regions and call the user provided callback for each
    /// entry. Stop when out of entries or the callback returns false.
    ///
    /// # Warning
    /// It is absolutely required that the user callback must not call into any other
    /// `RegionAllocator` public APIs, and should likely not acquire any locks of any
    /// kind. This method cannot protect against deadlocks and lock inversions that
    /// are possible by acquiring the allocation lock before calling the user provided
    /// callback. Because `KMutex` is not recursive, calling back into the allocator
    /// from within the callback will deadlock.
    pub fn walk_allocated_regions<F>(&self, mut cb: F)
    where
        F: FnMut(&Region) -> bool,
    {
        lock!(let guard = self.lock_mu());
        for region in guard.fields().allocated_regions_by_base.iter() {
            if !cb(region) {
                break;
            }
        }
    }

    /// Walk the available regions and call the user provided callback for each
    /// entry. Stop when out of entries or the callback returns false.
    ///
    /// # Warning
    /// It is absolutely required that the user callback must not call into any other
    /// `RegionAllocator` public APIs, and should likely not acquire any locks of any
    /// kind. This method cannot protect against deadlocks and lock inversions that
    /// are possible by acquiring the allocation lock before calling the user provided
    /// callback. Because `KMutex` is not recursive, calling back into the allocator
    /// from within the callback will deadlock.
    pub fn walk_available_regions<F>(&self, mut cb: F)
    where
        F: FnMut(&Region) -> bool,
    {
        lock!(let guard = self.lock_mu());
        for region in guard.fields().avail_regions_by_base.iter() {
            if !cb(region) {
                break;
            }
        }
    }

    // Private helpers

    fn add_subtract_sanity_check_locked_mut(
        &self,
        allocated_tree: &mut WavlTree<u64, NonNull<Region>, SortByBase, TrackingSize>,
        region: &RegionSpan,
    ) -> Result<(), Status> {
        if self.intersects_locked(allocated_tree, region)? {
            Err(Status::INVALID_ARGS)
        } else {
            Ok(())
        }
    }

    /// Release an allocated region back into the available pool.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `region_ptr` points to a valid `Region` that is currently in the
    /// allocated set.
    unsafe fn release_region(&self, region_ptr: NonNull<Region>) {
        lock!(let mut guard = self.lock_mu());
        let mut fields = guard.as_mut().fields_mut();

        // SAFETY: `region_ptr` is guaranteed to be a valid pointer in the allocated set.
        let region = unsafe { region_ptr.as_ref() };
        let removed = fields.allocated_regions_by_base.erase(&region.base());
        debug_assert!(removed.is_some());

        self.add_region_to_avail_locked(&mut fields, region_ptr, AllowOverlap::No);
    }

    fn add_region_to_avail_locked(
        &self,
        fields: &mut RegionAllocatorMuFieldsMut<'_>,
        region_ptr: NonNull<Region>,
        allow_overlap: AllowOverlap,
    ) {
        // SAFETY: `region_ptr` is a valid pointer to a `Region`.
        let region = unsafe { region_ptr.as_ref() };
        let mut region_base = region.base();
        let mut region_end = region_base + region.size();
        let original_region_base = region_base;

        {
            let mut before_cursor = fields.avail_regions_by_base.upper_bound(&region_base);
            before_cursor.move_prev();

            if before_cursor.get().is_some() {
                let before = before_cursor.get().unwrap();
                let before_end = before.base() + before.size();
                let should_merge = match allow_overlap {
                    AllowOverlap::Yes => before_end >= region_base,
                    AllowOverlap::No => before_end == region_base,
                };
                if should_merge {
                    region_end = core::cmp::max(region_end, before_end);
                    region_base = before.base();

                    let removed_ptr = before_cursor.erase().unwrap();
                    // SAFETY: `removed_ptr` is a valid pointer to a `Region`.
                    let key = unsafe { removed_ptr.as_ref().key_size };
                    fields.avail_regions_by_size.erase(&key);
                    // SAFETY: `removed_ptr` has been removed from all indices and is ready to be
                    // destroyed.
                    unsafe {
                        Self::destroy_region_raw(fields.region_pool, removed_ptr);
                    }
                }
            }
        } // before_cursor dropped

        let mut after_cursor = fields.avail_regions_by_base.upper_bound(&original_region_base);
        while after_cursor.get().is_some() {
            let after = after_cursor.get().unwrap();
            let should_merge = match allow_overlap {
                AllowOverlap::Yes => region_end >= after.base(),
                AllowOverlap::No => region_end == after.base(),
            };
            if !should_merge {
                break;
            }

            let after_end = after.base() + after.size();
            region_end = core::cmp::max(region_end, after_end);

            let removed_ptr = after_cursor.erase().unwrap();
            // SAFETY: `removed_ptr` is a valid pointer to a `Region`.
            let key = unsafe { removed_ptr.as_ref().key_size };
            fields.avail_regions_by_size.erase(&key);
            // SAFETY: `removed_ptr` has been removed from all indices and is ready to be destroyed.
            unsafe {
                Self::destroy_region_raw(fields.region_pool, removed_ptr);
            }

            if allow_overlap != AllowOverlap::Yes {
                break;
            }
        }

        // SAFETY: `region_ptr` is a valid pointer to a `Region`. Exclusivity is guaranteed
        // because the region is not currently in any of the allocator's trees (it is either
        // newly allocated or has been erased from the allocated tree in `release_region`).
        unsafe {
            let r = &mut *region_ptr.as_ptr();
            r.span.base = region_base;
            r.span.size = region_end - region_base;
            r.update_key();
        }

        // SAFETY: Inserting a valid region pointer back into the available indices is safe.
        unsafe {
            fields.avail_regions_by_base.insert_raw(region_ptr);
            fields.avail_regions_by_size.insert_raw(region_ptr);
        }
    }

    fn alloc_from_avail_locked(
        &self,
        fields: &mut RegionAllocatorMuFieldsMut<'_>,
        source_key: RegionKey,
        base: u64,
        size: u64,
    ) -> Result<UniquePtr<Region>, Status> {
        let mut source_cursor = fields.avail_regions_by_size.find_cursor(&source_key);
        let source_ref = source_cursor.get().ok_or(Status::BAD_STATE)?;
        let source_base = source_ref.base();
        let source_size = source_ref.size();

        let overhead = base - source_base;
        let leftover = source_size - size;

        let split_before = base != source_base;
        let split_after = overhead < leftover;

        if !split_before && !split_after {
            let region_ptr = source_cursor.erase().unwrap();
            // SAFETY: `region_ptr` is a valid pointer to a `Region`.
            let base = unsafe { region_ptr.as_ref().base() };
            fields.avail_regions_by_base.erase(&base);
            // SAFETY: Inserting `region_ptr` into the allocated tree is safe as it's been removed
            // from available trees.
            unsafe {
                fields.allocated_regions_by_base.insert_raw(region_ptr);
            }
            // SAFETY: `region_ptr` is a valid, uniquely owned allocation, so wrapping it in
            // `UniquePtr` is safe.
            Ok(unsafe { UniquePtr::from_raw(region_ptr.as_ptr()) })
        } else if !split_before {
            let after_region_ptr = source_cursor.erase().unwrap();
            // SAFETY: `after_region_ptr` is a valid pointer to a `Region`.
            let after_base = unsafe { after_region_ptr.as_ref().base() };
            fields.avail_regions_by_base.erase(&after_base);

            // The allocator lock is held. We allocate a new region raw.
            let before_region_ptr =
                self.create_region_raw(fields.region_pool, RegionSpan { base: after_base, size })?;

            // SAFETY: `after_region_ptr` is a valid pointer to a `Region`. Exclusivity is
            // guaranteed because we have erased the region from both the size and base available
            // trees (since its base address is changing).
            unsafe {
                let after_region = &mut *after_region_ptr.as_ptr();

                after_region.span.base += size;
                after_region.span.size -= size;
                after_region.update_key();
            }

            // SAFETY: Re-inserting `after_region_ptr` back into available indices and inserting
            // `before_region_ptr` into the allocated index is safe.
            unsafe {
                fields.avail_regions_by_size.insert_raw(after_region_ptr);
                fields.avail_regions_by_base.insert_raw(after_region_ptr);
                fields.allocated_regions_by_base.insert_raw(before_region_ptr);
            }
            // SAFETY: `before_region_ptr` is a valid, uniquely owned allocation, so wrapping it in
            // `UniquePtr` is safe.
            Ok(unsafe { UniquePtr::from_raw(before_region_ptr.as_ptr()) })
        } else if !split_after {
            let before_region_ptr = source_cursor.erase().unwrap();

            // The allocator lock is held. We allocate a new region raw.
            let after_region_ptr =
                self.create_region_raw(fields.region_pool, RegionSpan { base, size })?;

            // SAFETY: `before_region_ptr` is a pointer to a valid `Region`. Exclusivity is
            // guaranteed because we hold the allocator lock, and although the region remains in
            // `avail_regions_by_base` (its base address is unchanged), we have erased it from
            // `avail_regions_by_size` and will not access it through the base tree during mutation.
            unsafe {
                let before_region = &mut *before_region_ptr.as_ptr();

                before_region.span.size -= size;
                before_region.update_key();
            }

            // SAFETY: Re-inserting `before_region_ptr` back into available size index and inserting
            // `after_region_ptr` into the allocated index is safe.
            unsafe {
                fields.avail_regions_by_size.insert_raw(before_region_ptr);
                fields.allocated_regions_by_base.insert_raw(after_region_ptr);
            }
            // SAFETY: `after_region_ptr` is a valid, uniquely owned allocation, so wrapping it in
            // `UniquePtr` is safe.
            Ok(unsafe { UniquePtr::from_raw(after_region_ptr.as_ptr()) })
        } else {
            let before_region_ptr = source_cursor.erase().unwrap();
            // SAFETY: `before_region_ptr` is a valid pointer.
            let before_base = unsafe { before_region_ptr.as_ref().base() };
            let before_size = unsafe { before_region_ptr.as_ref().size() };

            let region_base = before_base + overhead;
            let region_size = size;

            // The allocator lock is held. We allocate two new regions raw.
            let region_ptr = self.create_region_raw(
                fields.region_pool,
                RegionSpan { base: region_base, size: region_size },
            )?;
            let after_region_ptr = self.create_region_raw(
                fields.region_pool,
                RegionSpan { base: region_base + region_size, size: before_size - size - overhead },
            )?;

            // SAFETY: `before_region_ptr` is a valid pointer to a `Region`. Exclusivity is
            // guaranteed because we hold the allocator lock, and although the region remains in
            // `avail_regions_by_base` (its base address is unchanged), we have erased it from
            // `avail_regions_by_size` and will not access it through the base tree during mutation.
            unsafe {
                let before_region = &mut *before_region_ptr.as_ptr();

                before_region.span.size = overhead;
                before_region.update_key();
            }

            // SAFETY: Re-inserting the split regions `before_region_ptr` and `after_region_ptr`
            // back into available indices, and inserting `region_ptr` into the allocated index is
            // safe.
            unsafe {
                fields.avail_regions_by_size.insert_raw(before_region_ptr);
                fields.avail_regions_by_size.insert_raw(after_region_ptr);
                fields.avail_regions_by_base.insert_raw(after_region_ptr);
                fields.allocated_regions_by_base.insert_raw(region_ptr);
            }
            // SAFETY: `region_ptr` is a valid, uniquely owned allocation, so wrapping it in
            // `UniquePtr` is safe.
            Ok(unsafe { UniquePtr::from_raw(region_ptr.as_ptr()) })
        }
    }

    fn intersects_locked(
        &self,
        tree: &mut WavlTree<u64, NonNull<Region>, SortByBase, TrackingSize>,
        region: &RegionSpan,
    ) -> Result<bool, Status> {
        region.validate()?;

        let mut iter = tree.lower_bound(&region.base);
        if let Some(current) = iter.get()
            && current.base() - region.base < region.size
        {
            return Ok(true);
        }

        iter.move_prev();
        if let Some(prev) = iter.get()
            && region.base - prev.base() < prev.size()
        {
            return Ok(true);
        }

        Ok(false)
    }

    fn contained_by_locked(
        &self,
        tree: &mut WavlTree<u64, NonNull<Region>, SortByBase, TrackingSize>,
        region: &RegionSpan,
    ) -> Result<bool, Status> {
        region.validate()?;
        let region_end = region.end()?;

        let mut iter = tree.upper_bound(&region.base);
        iter.move_prev();

        if let Some(r) = iter.get() {
            let r_end = r.base() + r.size();
            if region.base >= r.base() && region_end <= r_end {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Create a region by allocating it from the current RegionPool, or from the
    /// heap if we have no assigned region pool.
    ///
    /// The allocator lock must be held when calling this function.
    fn create_region_raw(
        &self,
        region_pool: &mut Option<RefPtr<RegionPool>>,
        span: RegionSpan,
    ) -> Result<NonNull<Region>, Status> {
        let region = Region::new(span, NonNull::from(self));
        if let Some(pool) = region_pool {
            let ptr = pool.allocator.alloc_raw().map_err(|_| Status::NO_MEMORY)?;
            // SAFETY: `ptr` is verified to be valid and uninitialized. Writing to it
            // initializes the slot without dropping uninitialized memory.
            unsafe {
                core::ptr::write(ptr.as_ptr(), region);
            }
            Ok(ptr)
        } else {
            let boxed = Box::try_new(region).map_err(|_| Status::NO_MEMORY)?;
            let raw = Box::into_raw(boxed);
            // SAFETY: `raw` is a valid non-null pointer returned by `Box::into_raw`.
            Ok(unsafe { NonNull::new_unchecked(raw) })
        }
    }

    /// Destroy a region by either returning it to the current RegionPool, or to
    /// the heap if we have no assigned region pool.
    ///
    /// # Safety
    /// The caller must ensure that `region_ptr` points to a valid `Region` that
    /// is no longer in use (i.e. has been removed from all lists and indices) and
    /// was allocated by the allocator context associated with `region_pool`.
    unsafe fn destroy_region_raw(
        region_pool: &mut Option<RefPtr<RegionPool>>,
        region_ptr: NonNull<Region>,
    ) {
        if let Some(pool) = region_pool {
            // SAFETY: `region_ptr` was allocated from `pool.allocator`.
            // We drop it in place, then return the raw storage to the slab allocator's free list.
            unsafe {
                core::ptr::drop_in_place(region_ptr.as_ptr());
                pool.allocator.return_to_free_list(region_ptr);
            }
        } else {
            // SAFETY: `region_ptr` was allocated as a heap-allocated `Box<Region>`.
            // Reconstructing the `Box` from the raw pointer allows its destructor to
            // automatically drop the inner `Region` and deallocate the memory correctly.
            unsafe {
                let _ = Box::from_raw(region_ptr.as_ptr());
            }
        }
    }
}

#[pin_init::pinned_drop]
impl pin_init::PinnedDrop for RegionAllocator {
    fn drop(self: core::pin::Pin<&mut Self>) {
        // SAFETY: We can obtain a mutable reference to the fields during drop because we are in
        // the drop implementation, and no other references can exist.
        let this = unsafe { self.get_unchecked_mut() };
        let allocated_regions_by_base = this.allocated_regions_by_base.get_inner_mut();
        let avail_regions_by_base = this.avail_regions_by_base.get_inner_mut();
        let avail_regions_by_size = this.avail_regions_by_size.get_inner_mut();
        let region_pool = this.region_pool.get_inner_mut();

        debug_assert!(allocated_regions_by_base.is_empty());
        debug_assert_eq!(avail_regions_by_base.len(), avail_regions_by_size.len());

        avail_regions_by_base.clear();
        while let Some(region_ptr) = avail_regions_by_size.pop_front() {
            // SAFETY: Popping the regions and destroying them is safe because the allocator itself
            // is being dropped, and no other references to these regions exist.
            unsafe {
                Self::destroy_region_raw(region_pool, region_ptr);
            }
        }
    }
}

#[cfg(test)]
mod tests;
