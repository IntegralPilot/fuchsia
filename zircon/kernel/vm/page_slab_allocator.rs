// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! # Porting Note
//!
//! The C++ and Rust versions of `PageSlabAllocator` and its associated types are not layout
//! identical.

use crate::kernel::types::VAddr;
use crate::vm::page::{VmPage, VmPageDoublyLinkedList, VmPagePtr, VmPageSlabState};
use crate::vm::page_state::VmPageState;
use crate::vm::{heap, physmap, pmm};
use core::alloc::Layout;
use core::convert::Infallible;
use core::error::Error;
use core::ffi::c_void;
use core::pin::Pin;
use core::ptr::{self, NonNull};
use core::{fmt, mem};
use fbl::DoublyLinkedListContainable;
use page_bindings::vm_page_state;
use pin_init::{PinInit, pin_data, pin_init};
use zx_status::Status;

#[derive(Debug)]
pub struct SlabAllocationError(());

impl fmt::Display for SlabAllocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("failed to allocate slab")
    }
}

impl Error for SlabAllocationError {}

impl From<SlabAllocationError> for Status {
    fn from(_: SlabAllocationError) -> Self {
        Status::NO_MEMORY
    }
}

pub trait SlabProvider {
    fn alloc_slab(self: Pin<&mut Self>) -> Result<NonNull<VmPage>, SlabAllocationError>;
    /// # Safety
    ///
    /// The argument `slab` must have been previously returned by `alloc_slab`. It must not have
    /// been freed.
    unsafe fn free_slab(self: Pin<&mut Self>, slab: NonNull<VmPage>);
}

pub struct BaseSlabProvider {}

impl SlabProvider for BaseSlabProvider {
    fn alloc_slab(self: Pin<&mut Self>) -> Result<NonNull<VmPage>, SlabAllocationError> {
        let (page, _pa) = pmm::alloc_page(0).map_err(|_status| SlabAllocationError(()))?;
        let page = page.as_non_null();
        // SAFETY: We own this freshly allocated page.
        unsafe {
            page.as_ref().set_state(VmPageState(vm_page_state::SLAB));
            let slab = &mut *slab_state_mut(page);

            // There is enough space to store a cookie per allocation in the slab, so
            // amortize it and record a per slab cookie. On average this should have every different
            // call site using this allocator to get proportional blame.
            slab.profile_cookie = heap::profile_track_alloc(page::SIZE);
        }
        Ok(page)
    }

    unsafe fn free_slab(self: Pin<&mut Self>, slab: NonNull<VmPage>) {
        // SAFETY: Caller is giving us ownership of this page.
        unsafe {
            debug_assert_eq!(slab.as_ref().state(), VmPageState(vm_page_state::SLAB));

            let page = &*slab_state_mut(slab);
            heap::profile_track_free(page.profile_cookie, page::SIZE);

            // SAFETY: We have ownership over this page.
            pmm::free_page(VmPagePtr::new(slab));
        }
    }
}

/// # Safety
///
/// The caller must possess conceptual ownership of `slab` and ensure that `state_union` is
/// in the `SLAB` state, and that accessing this subfield does not race with concurrent access.
unsafe fn slab_state_mut(slab: NonNull<VmPage>) -> *mut VmPageSlabState {
    // SAFETY: Safety deferred to caller per function safety preconditions.
    unsafe { &raw mut (*(*slab.as_ptr()).state_union.get()).slab }
}

union Entry<const ALLOC_SIZE: usize> {
    next: u32,
    storage: [u8; ALLOC_SIZE],
}

/// Simple slab allocator that uses a `VmPage` as its slab to perform allocations out of. This makes
/// the allocator only suitable for small allocations, preferably ones that divide evenly into a
/// page.
///
/// All per-slab metadata is stored in the `VmPage` itself and, by default, the only dependency of
/// the allocator is the `pmm` to allocate and free pages from. The heap is not needed for any other
/// metadata allocations.
///
/// Free regions are tracked in a two level list with each slab having an internal list of free
/// regions, and the allocator itself having a list of slabs that have at least one free region.
///
/// Slabs that become fully empty are able to be returned to the `pmm`, although allocations
/// cannot be moved between slabs, so fragmentation can still occur.
///
/// The contained slab provider can be edited to control the actual allocation and freeing of
/// the slabs themselves.
///
/// This class is not thread safe.
///
/// See the module-level documentation.
#[pin_data(PinnedDrop)]
pub struct PageSlabAllocator<const ALLOC_SIZE: usize, A> {
    #[pin]
    full_slabs: VmPageDoublyLinkedList,
    #[pin]
    available_slabs: VmPageDoublyLinkedList,
    /// Track the total number of allocated slabs (i.e. pages), both full and available.
    allocated_slabs: usize,
    /// This is permitted to be an address-sensitive type.
    #[pin]
    inner: A,
}

impl<const ALLOC_SIZE: usize, A> PageSlabAllocator<ALLOC_SIZE, A> {
    pub const ALLOCS_PER_SLAB: usize = page::SIZE / ALLOC_SIZE;
    const END_OF_LIST: u32 = u32::MAX;

    pub fn new_with(inner: impl PinInit<A, Infallible>) -> impl PinInit<Self, Infallible> {
        const {
            assert!(mem::size_of::<Entry<ALLOC_SIZE>>() == ALLOC_SIZE);
            assert!(ALLOC_SIZE < page::SIZE);
        }

        pin_init!(Self {
            full_slabs <- VmPageDoublyLinkedList::new(),
            available_slabs <- VmPageDoublyLinkedList::new(),
            allocated_slabs: 0,
            inner <- inner,
        })
    }

    /// Allocates an area of uninitialized memory of AllocSize and returns a pointer to it, or
    /// returns an error.
    pub fn allocate_bytes(self: Pin<&mut Self>) -> Result<NonNull<c_void>, SlabAllocationError>
    where
        A: SlabProvider,
    {
        let entry = self.allocate()?;
        Ok(entry.cast())
    }

    /// Allocates an area of uninitialized memory capable of holding a single object of type `T`.
    /// This is largely a convenience wrapper around `allocate_bytes` that validates `T` is
    /// compatible with the size and alignment of the allocations.
    ///
    /// Returns an error on failure.
    pub fn allocate_object<T>(self: Pin<&mut Self>) -> Result<NonNull<T>, SlabAllocationError>
    where
        A: SlabProvider,
    {
        const {
            let layout = Layout::new::<T>();
            // T must fit inside the size of the allocation.
            assert!(layout.size() <= ALLOC_SIZE);
            // Allocations must have at least equivalent alignment.
            assert!(layout.align() <= Self::entry_align());
        }
        let entry = self.allocate()?;
        Ok(entry.cast())
    }

    /// Deallocates the storage referenced by `ptr`, which must be a pointer obtained by an earlier
    /// call to `allocate_object` or `allocate_bytes`.
    ///
    /// # Safety
    ///
    /// `ptr` must be a pointer obtained from this allocator that has not already been deallocated.
    pub unsafe fn deallocate_bytes(self: Pin<&mut Self>, ptr: NonNull<c_void>)
    where
        A: SlabProvider,
    {
        // SAFETY: Caller guarantees `ptr` was allocated by this allocator and not yet freed.
        unsafe {
            self.free(ptr);
        }
    }

    /// Typed convenience wrapper around `deallocate_bytes`. Assumes the object at `ptr` has already
    /// been destructed.
    ///
    /// # Safety
    ///
    /// `ptr` must be a pointer obtained from this allocator that has not already been deallocated.
    pub unsafe fn deallocate_object<T>(self: Pin<&mut Self>, ptr: NonNull<T>)
    where
        A: SlabProvider,
    {
        let ptr: NonNull<c_void> = ptr.cast();
        // SAFETY: Caller attests to the preconditions for free.
        unsafe {
            self.free(ptr);
        }
    }

    pub fn allocated_slabs(&self) -> usize {
        self.allocated_slabs
    }

    /// Helper to return the number of slabs this allocator would allocate to store the specified
    /// number of allocations.
    pub const fn slabs_required(num_allocs: usize) -> usize {
        num_allocs.div_ceil(Self::ALLOCS_PER_SLAB)
    }

    const fn entry_align() -> usize {
        1usize << mem::size_of::<Entry<ALLOC_SIZE>>().trailing_zeros()
    }

    fn alloc_to_slab<T>(ptr: NonNull<T>) -> (NonNull<VmPage>, u32) {
        let ptr: usize = ptr.addr().into();
        let offset = ptr % page::SIZE;
        let page = pmm::paddr_to_vm_page(physmap::physmap_to_paddr(VAddr(ptr)))
            .expect("allocated pointer maps to valid vm_page");
        let page: NonNull<VmPage> = page.as_non_null();
        // SAFETY: `page` is a valid page allocated by this allocator.
        let page_ref = unsafe { page.as_ref() };
        assert_eq!(page_ref.state(), VmPageState(vm_page_state::SLAB));
        debug_assert!(page_ref.get_node().in_container());
        debug_assert!(offset.is_multiple_of(ALLOC_SIZE));
        (page, (offset / ALLOC_SIZE) as u32)
    }

    /// # Safety
    ///
    /// `slab` must have come from this allocator.
    unsafe fn get_entry(slab: NonNull<VmPage>, index: u32) -> NonNull<Entry<ALLOC_SIZE>> {
        // SAFETY: Caller guarantees `slab` came from this allocator.
        let page = unsafe { slab.as_ref() };
        assert_eq!(page.state(), VmPageState(vm_page_state::SLAB));
        debug_assert!((index as usize) < Self::ALLOCS_PER_SLAB);
        let ptr = physmap::paddr_to_physmap(page.paddr());
        let ptr = ptr::with_exposed_provenance_mut::<Entry<ALLOC_SIZE>>(ptr.0);
        // SAFETY: `slab` has been allocated in the physmap and `index` is within the slab bounds.
        unsafe { NonNull::new_unchecked(ptr.add(index as usize)) }
    }

    fn add_slab(self: Pin<&mut Self>) -> Result<(), SlabAllocationError>
    where
        A: SlabProvider,
    {
        let this = self.project();
        // Allocate a new slab.
        let slab = this.inner.alloc_slab()?;

        // SAFETY: We own the freshly allocated slab and `slab_state` is valid for writes.
        unsafe {
            let slab_state = slab_state_mut(slab);
            (*slab_state).free_slot = Self::END_OF_LIST;
            (*slab_state).peak_allocated = 0;
            (*slab_state).allocated = 0;
        }

        // Insert it into the available slabs.
        // SAFETY: `slab` is valid for insertion and not in any other list.
        unsafe {
            this.available_slabs.get_unchecked_mut().push_front_raw(slab);
        }

        *this.allocated_slabs += 1;
        Ok(())
    }

    fn allocate(mut self: Pin<&mut Self>) -> Result<NonNull<Entry<ALLOC_SIZE>>, SlabAllocationError>
    where
        A: SlabProvider,
    {
        // See if there are any slabs available.
        if self.available_slabs.is_empty() {
            self.as_mut().add_slab()?;
        }

        let mut this = self.project();

        // SAFETY: `available_slabs` is pinned and not moved.
        let mut cursor =
            unsafe { this.available_slabs.as_mut().get_unchecked_mut().cursor_front_mut() };

        let page = NonNull::from(cursor.get().expect("available slabs list has elements"));

        let entry;
        // SAFETY: `page` is valid for reads.
        let free_slot = unsafe { (*slab_state_mut(page)).free_slot };

        if free_slot == Self::END_OF_LIST {
            // SAFETY: `page` is valid for reads.
            let peak = unsafe { (*slab_state_mut(page)).peak_allocated };
            debug_assert!((peak as usize) < Self::ALLOCS_PER_SLAB);

            // SAFETY: `page` is valid for entry lookup and `peak` is within bounds.
            entry = unsafe { Self::get_entry(page, peak) };

            // SAFETY: `page` is valid for writes.
            unsafe {
                (*slab_state_mut(page)).peak_allocated += 1;
            }
        } else {
            // SAFETY: `page` is valid for entry lookup and `free_slot` is within bounds.
            entry = unsafe { Self::get_entry(page, free_slot) };

            // SAFETY: `page` is valid for writes and `entry` is valid for reads.
            unsafe {
                (*slab_state_mut(page)).free_slot = (*entry.as_ptr()).next;
            }
        }

        // SAFETY: `page` is valid for writes.
        unsafe {
            (*slab_state_mut(page)).allocated += 1;
        }

        // SAFETY: `page` is valid for reads.
        let free_slot = unsafe { (*slab_state_mut(page)).free_slot };
        // SAFETY: `page` is valid for reads.
        let peak = unsafe { (*slab_state_mut(page)).peak_allocated };
        // SAFETY: `page` is valid for reads.
        let allocated = unsafe { (*slab_state_mut(page)).allocated };

        if free_slot == Self::END_OF_LIST && (peak as usize) == Self::ALLOCS_PER_SLAB {
            debug_assert_eq!(allocated as usize, Self::ALLOCS_PER_SLAB);
            let page = cursor.erase().unwrap();
            // SAFETY: `full_slabs` is pinned and not moved.
            let full_slabs = unsafe { this.full_slabs.get_unchecked_mut() };
            // SAFETY: `page` is valid for insertion and not in any list.
            unsafe {
                full_slabs.push_front_raw(page);
            }
        } else {
            debug_assert!((allocated as usize) < Self::ALLOCS_PER_SLAB);
        }

        Ok(entry)
    }

    unsafe fn free(mut self: Pin<&mut Self>, ptr: NonNull<c_void>)
    where
        A: SlabProvider,
    {
        let this = self.as_mut().project();
        // SAFETY: `available_slabs` is pinned and not moved.
        let available_slabs = unsafe { this.available_slabs.get_unchecked_mut() };
        // SAFETY: `full_slabs` is pinned and not moved.
        let full_slabs = unsafe { this.full_slabs.get_unchecked_mut() };

        // Lookup the slab this was allocated in.
        let (slab, index) = Self::alloc_to_slab(ptr);

        // SAFETY: `slab` is valid for reads.
        let free_slot = unsafe { (*slab_state_mut(slab)).free_slot };
        // SAFETY: `slab` is valid for reads.
        let allocated = unsafe { (*slab_state_mut(slab)).allocated };

        // This will only catch the most egregious kinds of double-frees, but is better than
        // nothing.
        debug_assert_ne!(free_slot, index);
        debug_assert!(allocated > 0);

        if allocated == 1 {
            // Slab has become empty, can free it.
            // SAFETY: `slab` is valid for removal and present in `available_slabs`.
            unsafe {
                available_slabs.erase(slab.as_ref());
            }
            *this.allocated_slabs -= 1;
            // SAFETY: `slab` is valid to free and not in any list.
            unsafe {
                this.inner.free_slab(slab);
            }
            return;
        }

        if allocated as usize == Self::ALLOCS_PER_SLAB {
            // Slab is going from full to having space available, move to the correct list. We place
            // at the back of the list to encourage allocations, which happen on the head, to fill
            // up a page instead of constantly bouncing allocations into different, partially full,
            // pages.
            // SAFETY: `slab` is valid for list transfer from `full_slabs` to `available_slabs`.
            unsafe {
                full_slabs.erase(slab.as_ref());
                available_slabs.push_back_raw(slab);
            }
        }

        // Update the free list for this slab.

        // SAFETY: `slab` is valid for entry lookup and `index` is within bounds.
        let entry = unsafe { Self::get_entry(slab, index) };

        // SAFETY: `slab_state` and `entry` are valid for writes.
        unsafe {
            let slab_state = slab_state_mut(slab);
            (*slab_state).allocated -= 1;

            (*entry.as_ptr()).next = (*slab_state).free_slot;
            (*slab_state).free_slot = index;
        }
    }

    /// Support for assigning a numerical ID to each allocation in lieu of a pointer, and
    /// converting between them. These conversion methods are available when provider `A`
    /// implements [`IdToSlab`], under the precondition that the provider assigns each allocated
    /// slab a unique ID such that `id * Self::ALLOCS_PER_SLAB` fits within a `u32`.
    ///
    /// The size of the ID is fixed at 32 bits, and not variable, since a 16-bit limit is
    /// unlikely to be useful in practice, and a 64-bit limit has no value since the pointer is
    /// already a 64-bit ID.
    pub fn alloc_to_id<T>(&self, ptr: NonNull<T>) -> u32
    where
        A: IdToSlab<ALLOC_SIZE>,
    {
        let (slab, slab_index) = Self::alloc_to_slab(ptr);
        // SAFETY: `slab` is valid for reads.
        let page = unsafe { &*slab_state_mut(slab) };
        (page.id * Self::ALLOCS_PER_SLAB as u32) + slab_index
    }

    pub fn id_to_alloc<T>(&self, id: u32) -> NonNull<T>
    where
        A: IdToSlab<ALLOC_SIZE>,
    {
        let slab = self.inner.id_to_slab(id / Self::ALLOCS_PER_SLAB as u32);
        // SAFETY: `slab` is valid for entry lookup.
        let alloc = unsafe { Self::get_entry(slab, id % Self::ALLOCS_PER_SLAB as u32) };
        alloc.cast()
    }

    pub fn provider(&self) -> &A {
        &self.inner
    }

    pub fn debug_free_all_slabs(self: Pin<&mut Self>) {
        let this = self.project();
        for p in this.full_slabs.iter() {
            // SAFETY: `p` is valid for reads.
            let slab = unsafe { &*slab_state_mut(NonNull::from(p)) };
            heap::profile_track_free(slab.profile_cookie, page::SIZE);
        }
        for p in this.available_slabs.iter() {
            // SAFETY: `p` is valid for reads.
            let slab = unsafe { &*slab_state_mut(NonNull::from(p)) };
            heap::profile_track_free(slab.profile_cookie, page::SIZE);
        }
        // SAFETY: Slabs in full_slabs and available_slabs are valid allocated PMM pages.
        // We do not move the DoublyLinkedList containers out of their pinned location.
        unsafe {
            pmm::free_list(this.full_slabs);
            pmm::free_list(this.available_slabs);
        }
    }
}

#[pin_init::pinned_drop]
impl<const ALLOC_SIZE: usize, A> PinnedDrop for PageSlabAllocator<ALLOC_SIZE, A> {
    fn drop(self: Pin<&mut Self>) {
        assert!(self.full_slabs.is_empty());
        assert!(self.available_slabs.is_empty());
    }
}

pub trait IdToSlab<const ALLOC_SIZE: usize> {
    fn id_to_slab(&self, slab_id: u32) -> NonNull<VmPage>;
}

/// A [`SlabProvider`] and [`IdToSlab`] implementation that supports a fixed number of slabs,
/// as determined by `MAX_ALLOCS`, and statically pre-allocates an array of `SLABS_REQUIRED`
/// slab pointers (8 bytes per slab).
///
/// The caller must specify `SLABS_REQUIRED` even though it's a function of `MAX_ALLOCS` and
/// `ALLOC_SIZE`. Specifically, it must be `MAX_ALLOCS.div_ceil(page::SIZE / ALLOC_SIZE)`. We will
/// be able to compute the size of this array using an expression over the generic parameters with
/// the `generic_const_exprs` feature, when it becomes available.
#[pin_data(PinnedDrop)]
pub struct FixedIdSlabProvider<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    A = BaseSlabProvider,
> {
    slabs: [Option<NonNull<VmPage>>; SLABS_REQUIRED],
    #[pin]
    inner: A,
}

impl<const ALLOC_SIZE: usize, const MAX_ALLOCS: usize, const SLABS_REQUIRED: usize>
    FixedIdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, BaseSlabProvider>
{
    pub fn new() -> impl PinInit<Self, Infallible> {
        const {
            assert!(ALLOC_SIZE < page::SIZE);
            assert!(mem::size_of::<Entry<ALLOC_SIZE>>() == ALLOC_SIZE);
            assert!(ALLOC_SIZE >= mem::size_of::<u32>());

            let allocs_per_slab = page::SIZE / ALLOC_SIZE;
            let slabs_required =
                PageSlabAllocator::<ALLOC_SIZE, BaseSlabProvider>::slabs_required(MAX_ALLOCS);
            assert!(MAX_ALLOCS.is_multiple_of(allocs_per_slab));
            assert!(SLABS_REQUIRED == slabs_required);
        }

        pin_init::pin_init!(Self {
            slabs: [None; SLABS_REQUIRED],
            inner <- pin_init::init!(BaseSlabProvider {}),
        })
    }
}

impl<const ALLOC_SIZE: usize, const MAX_ALLOCS: usize, const SLABS_REQUIRED: usize, A> SlabProvider
    for FixedIdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, A>
where
    A: SlabProvider,
{
    fn alloc_slab(self: Pin<&mut Self>) -> Result<NonNull<VmPage>, SlabAllocationError> {
        let this = self.project();
        // Find an unused slot/id.
        let id = this.slabs.iter().position(|p| p.is_none()).ok_or(SlabAllocationError(()))?;
        let slab = this.inner.alloc_slab()?;

        // SAFETY: We own this freshly allocated page.
        let page = unsafe { slab.as_ref() };
        unsafe { page.set_state(VmPageState(vm_page_state::SLAB)) };

        // SAFETY: `slab` is valid for writes.
        let page = unsafe { &mut *slab_state_mut(slab) };
        page.id = id as u32;
        this.slabs[id] = Some(slab);
        Ok(slab)
    }

    unsafe fn free_slab(self: Pin<&mut Self>, slab: NonNull<VmPage>) {
        // SAFETY: Caller promises we own slab now.
        let page = unsafe { slab.as_ref() };
        debug_assert_eq!(page.state(), VmPageState(vm_page_state::SLAB));

        // SAFETY: `slab` is valid for reads.
        let page = unsafe { &*slab_state_mut(slab) };
        let id = page.id as usize;
        let this = self.project();
        debug_assert_eq!(this.slabs[id], Some(slab));
        this.slabs[id] = None;

        // SAFETY: Caller promises we own slab now.
        unsafe {
            this.inner.free_slab(slab);
        }
    }
}

#[pin_init::pinned_drop]
impl<const ALLOC_SIZE: usize, const MAX_ALLOCS: usize, const SLABS_REQUIRED: usize, A> PinnedDrop
    for FixedIdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, A>
{
    fn drop(self: Pin<&mut Self>) {
        let all_free = self.slabs.iter().all(|slab| slab.is_none());
        assert!(all_free);
    }
}

impl<const ALLOC_SIZE: usize, const MAX_ALLOCS: usize, const SLABS_REQUIRED: usize, A>
    IdToSlab<ALLOC_SIZE> for FixedIdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, A>
{
    fn id_to_slab(&self, slab_id: u32) -> NonNull<VmPage> {
        let slab = self.slabs[slab_id as usize];
        debug_assert!(slab.is_some());
        slab.expect("slab for id is allocated")
    }
}

/// A [`PageSlabAllocator`] backed by [`FixedIdSlabProvider`] that guarantees all allocated IDs
/// are within `[0, MAX_ALLOCS)`.
///
/// To achieve this, `MAX_ALLOCS` must be an exact multiple of the number of allocations per slab
/// (`Self::ALLOCS_PER_SLAB`).
///
/// Due to the static overhead per potential slab required by [`FixedIdSlabProvider`], this object
/// can become quite large as `MAX_ALLOCS` grows. Users should check the resulting size of the
/// allocator based on `ALLOC_SIZE` and `MAX_ALLOCS` to ensure it is within acceptable limits.
///
/// In most cases [`IdSlabAllocator`] is preferable, as it allocates slab metadata dynamically.
pub type FixedIdSlabAllocator<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    A = BaseSlabProvider,
> = PageSlabAllocator<ALLOC_SIZE, FixedIdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, A>>;

impl<const ALLOC_SIZE: usize, const MAX_ALLOCS: usize, const SLABS_REQUIRED: usize>
    FixedIdSlabAllocator<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED>
{
    pub fn new() -> impl PinInit<Self, Infallible> {
        PageSlabAllocator::new_with(FixedIdSlabProvider::new())
    }
}

/// See the comment over [`FixedIdSlabProvider`] for more about the only permissible values of
/// `SLABS_REQUIRED` and `SLAB_ID_SLABS_REQUIRED`.
#[pin_data]
pub struct IdSlabProvider<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    const SLAB_ID_SLABS_REQUIRED: usize,
    Slab = FixedIdSlabProvider<
        { mem::size_of::<NonNull<VmPage>>() },
        SLABS_REQUIRED,
        SLAB_ID_SLABS_REQUIRED,
        BaseSlabProvider,
    >,
    A = BaseSlabProvider,
> {
    #[pin]
    base: A,
    /// The slab id allocator both allocates our unique slab ids, and provides the storage / mapping
    /// from slab_id -> NonNull<VmPage>. Importantly it guarantees that its IDs are from
    /// [0, SLABS_REQUIRED), which we need to then guarantee that our final IDs are from
    /// [0, MAX_ALLOCS).
    #[pin]
    slab_id_allocator: PageSlabAllocator<{ mem::size_of::<NonNull<VmPage>>() }, Slab>,
}

impl<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    const SLAB_ID_SLABS_REQUIRED: usize,
    Slab,
    A,
> IdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, SLAB_ID_SLABS_REQUIRED, Slab, A>
{
    pub fn new_with(
        base: impl PinInit<A, Infallible>,
        slab_id_allocator: impl PinInit<
            PageSlabAllocator<{ mem::size_of::<NonNull<VmPage>>() }, Slab>,
            Infallible,
        >,
    ) -> impl PinInit<Self, Infallible> {
        const {
            let allocs_per_slab = page::SIZE / ALLOC_SIZE;
            let slabs_required =
                PageSlabAllocator::<ALLOC_SIZE, BaseSlabProvider>::slabs_required(MAX_ALLOCS);
            assert!(MAX_ALLOCS.is_multiple_of(allocs_per_slab));
            assert!(SLABS_REQUIRED == slabs_required);

            let id_alloc_size = mem::size_of::<NonNull<VmPage>>();
            let id_allocs_per_slab = page::SIZE / id_alloc_size;
            let id_slabs_required = PageSlabAllocator::<
                { mem::size_of::<NonNull<VmPage>>() },
                BaseSlabProvider,
            >::slabs_required(SLABS_REQUIRED);
            assert!(SLABS_REQUIRED.is_multiple_of(id_allocs_per_slab));
            assert!(SLAB_ID_SLABS_REQUIRED == id_slabs_required);
        }

        pin_init::pin_init!(Self {
            base <- base,
            slab_id_allocator <- slab_id_allocator,
        })
    }

    pub fn slab_id_allocator(
        &self,
    ) -> &PageSlabAllocator<{ mem::size_of::<NonNull<VmPage>>() }, Slab> {
        &self.slab_id_allocator
    }
}

impl<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    const SLAB_ID_SLABS_REQUIRED: usize,
    Slab,
    A,
> IdToSlab<ALLOC_SIZE>
    for IdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, SLAB_ID_SLABS_REQUIRED, Slab, A>
where
    Slab: IdToSlab<{ mem::size_of::<NonNull<VmPage>>() }>,
{
    fn id_to_slab(&self, slab_id: u32) -> NonNull<VmPage> {
        let ptr: NonNull<NonNull<VmPage>> = self.slab_id_allocator.id_to_alloc(slab_id);
        // SAFETY: `slab_id_allocator` stores NonNull<VmPage> at this slot.
        unsafe { ptr.read() }
    }
}

impl<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    const SLAB_ID_SLABS_REQUIRED: usize,
    Slab,
    A,
> SlabProvider
    for IdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, SLAB_ID_SLABS_REQUIRED, Slab, A>
where
    A: SlabProvider,
    Slab: SlabProvider + IdToSlab<{ mem::size_of::<NonNull<VmPage>>() }>,
{
    fn alloc_slab(self: Pin<&mut Self>) -> Result<NonNull<VmPage>, SlabAllocationError> {
        let mut this = self.project();
        // Allocate a new slot to store the slab from the id allocator.
        let slab_ref: NonNull<NonNull<VmPage>> =
            this.slab_id_allocator.as_mut().allocate_object()?;
        let slab: NonNull<VmPage> = this.base.as_mut().alloc_slab().inspect_err(|_| {
            // SAFETY: We just allocated slab_ref.
            unsafe { this.slab_id_allocator.as_mut().deallocate_object(slab_ref) };
        })?;
        // SAFETY: `slab_ref` is valid for writes.
        unsafe {
            slab_ref.write(slab);
        }
        // The object id of our slot is our slab id, allowing us to go from id -> NonNull<VmPage>
        // later.
        let slab_id = this.slab_id_allocator.as_mut().alloc_to_id(slab_ref);
        // SAFETY: `slab` is valid for writes.
        let page = unsafe { &mut *slab_state_mut(slab) };
        page.id = slab_id;
        Ok(slab)
    }

    unsafe fn free_slab(self: Pin<&mut Self>, slab: NonNull<VmPage>) {
        // Return the slot used to store slab to the slab id allocator.
        // SAFETY: Caller promises we own slab now and `slab` is valid for reads.
        let page = unsafe { &*slab_state_mut(slab) };
        let slab_id = page.id;

        let mut this = self.project();
        let slab_ref: NonNull<NonNull<VmPage>> = this.slab_id_allocator.id_to_alloc(slab_id);
        // SAFETY: `slab_ref` is valid for reads.
        debug_assert_eq!(unsafe { slab_ref.read() }, slab);
        // SAFETY: `slab_ref` was allocated by `this.slab_id_allocator`.
        unsafe {
            this.slab_id_allocator.as_mut().deallocate_object(slab_ref);
        }
        // SAFETY: `slab` was allocated by `this.base` and is owned by us.
        unsafe {
            this.base.free_slab(slab);
        }
    }
}

/// Slab allocator that can convert each allocation to/from a numerical ID, with the added guarantee
/// that all IDs fall within `[0, MAX_ALLOCS)`. This implies that only up to `MAX_ALLOCS` can be
/// allocated at one time. To achieve this it is a restriction that the maximum number of requested
/// allocations be a common multiple of the number of allocations per slab and per ID slab.
///
/// The properties of this allocator are otherwise the same as the `PageSlabAllocator`, with the
/// only dependency being direct PMM allocations, and slabs able to be cleaned up if they become
/// fully free.
///
/// This slab allocator uses the `FixedIdSlabAllocator` internally to allocate slab IDs by default.
/// This works as follows:
///
/// 1. Whenever this slab allocator allocates a slab, the `NonNull<VmPage>` pointing to that slab is
///    stored in the `FixedIdSlabAllocator`.
///
/// 2. By construction, each slab in the `FixedIdSlabAllocator` has an ID that we can convert into a
///    slab. Thus, using the `alloc_to_id` function allows us to convert the
///    `NonNull<NonNull<VmPage>>` into a stable ID that we can assign as the slab's ID in this
///    allocator.
///
/// We do this to minimize the amount of metadata needed to store the IDs. Notice that using a
/// `FixedIdSlabAllocator` directly would increase the allocation size by
/// `size_of::<NonNull<VmPage>>()` bytes for every slab we allocate. By introducing this level of
/// indirection, we are able to add a slab to the `FixedIdSlabAllocator` only when we have
/// `(size_of::<NonNull<VmPage>>() / page::SIZE)` slabs allocated in this allocator. We can fit
/// `(ALLOC_SIZE / page::SIZE)` allocations in each slab in this allocator, so this means that the
/// size of the underlying `FixedIdSlabAllocator` will grow at a rate of
/// `(size_of::<NonNull<VmPage>>() * ALLOC_SIZE) / (page::SIZE * page::SIZE)` bytes per allocation
/// added.
///
/// The rate of growth of the static metadata can be further reduced by adding additional levels of
/// indirection for the slab id allocation by overriding the `Slab` generic argument.
///
/// See the comment over [`FixedIdSlabProvider`] for more about the only permissible values of
/// `SLABS_REQUIRED` and `SLAB_ID_SLABS_REQUIRED`.
pub type IdSlabAllocator<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    const SLAB_ID_SLABS_REQUIRED: usize,
    Slab = FixedIdSlabProvider<
        { mem::size_of::<NonNull<VmPage>>() },
        SLABS_REQUIRED,
        SLAB_ID_SLABS_REQUIRED,
        BaseSlabProvider,
    >,
    A = BaseSlabProvider,
> = PageSlabAllocator<
    ALLOC_SIZE,
    IdSlabProvider<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, SLAB_ID_SLABS_REQUIRED, Slab, A>,
>;

impl<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    const SLAB_ID_SLABS_REQUIRED: usize,
>
    IdSlabAllocator<
        ALLOC_SIZE,
        MAX_ALLOCS,
        SLABS_REQUIRED,
        SLAB_ID_SLABS_REQUIRED,
        FixedIdSlabProvider<
            { mem::size_of::<NonNull<VmPage>>() },
            SLABS_REQUIRED,
            SLAB_ID_SLABS_REQUIRED,
            BaseSlabProvider,
        >,
        BaseSlabProvider,
    >
{
    pub fn new() -> impl PinInit<Self, Infallible> {
        PageSlabAllocator::new_with(IdSlabProvider::new_with(
            pin_init::init!(BaseSlabProvider {}),
            FixedIdSlabAllocator::new(),
        ))
    }
}

impl<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    const SLAB_ID_SLABS_REQUIRED: usize,
    Slab,
    A,
> IdSlabAllocator<ALLOC_SIZE, MAX_ALLOCS, SLABS_REQUIRED, SLAB_ID_SLABS_REQUIRED, Slab, A>
{
    /// Total memory usage, in bytes, including any unused portions of slabs, not including the size
    /// of `self`.
    pub fn memory_usage(&self) -> usize {
        (self.allocated_slabs() + self.provider().slab_id_allocator().allocated_slabs())
            * page::SIZE
    }
}

/// An example of overriding the `Slab` argument to provide an additional level of indirection for
/// the ID allocation, limiting the growth of metadata by another multiple of the `page::SIZE`.
///
/// See the comment over [`FixedIdSlabProvider`] for more about the only permissible values of
/// `SLABS_REQUIRED`, `LEVEL2_SLABS_REQUIRED` and `LEVEL2_SLAB_ID_SLABS_REQUIRED`.
pub type TwoLevelIdSlabAllocator<
    const ALLOC_SIZE: usize,
    const MAX_ALLOCS: usize,
    const SLABS_REQUIRED: usize,
    const LEVEL2_SLABS_REQUIRED: usize,
    const LEVEL2_SLAB_ID_SLABS_REQUIRED: usize,
    A = BaseSlabProvider,
> = PageSlabAllocator<
    ALLOC_SIZE,
    IdSlabProvider<
        ALLOC_SIZE,
        MAX_ALLOCS,
        SLABS_REQUIRED,
        LEVEL2_SLABS_REQUIRED,
        IdSlabProvider<
            { mem::size_of::<NonNull<VmPage>>() },
            SLABS_REQUIRED,
            LEVEL2_SLABS_REQUIRED,
            LEVEL2_SLAB_ID_SLABS_REQUIRED,
            FixedIdSlabProvider<
                { mem::size_of::<NonNull<VmPage>>() },
                LEVEL2_SLABS_REQUIRED,
                LEVEL2_SLAB_ID_SLABS_REQUIRED,
                BaseSlabProvider,
            >,
            BaseSlabProvider,
        >,
        A,
    >,
>;
