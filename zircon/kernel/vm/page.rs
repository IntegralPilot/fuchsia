// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::page_state::VmPageState;
use crate::kernel::percpu::PerCpu;
use crate::kernel::types::PAddr;
use bitflags::bitflags;
use core::cell::UnsafeCell;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU8, Ordering};
use page_bindings as bindings;

#[cfg(console_enabled)]
use crate::console_rust::console::{CMD_AVAIL_NORMAL, CmdArgs, static_command};

pub mod object {
    use page_bindings as bindings;

    pub const PIN_COUNT_BITS: u32 = bindings::VM_PAGE_OBJECT_PIN_COUNT_BITS;
    pub const MAX_PIN_COUNT: u32 = bindings::VM_PAGE_OBJECT_MAX_PIN_COUNT;
    pub const DIRTY_STATE_BITS: u32 = bindings::VM_PAGE_OBJECT_DIRTY_STATE_BITS;
    pub const MAX_DIRTY_STATES: u32 = bindings::VM_PAGE_OBJECT_MAX_DIRTY_STATES;
    pub const DIRTY_STATES_MASK: u32 = bindings::VM_PAGE_OBJECT_DIRTY_STATES_MASK;
}

bitflags! {
    /// Flags packed into a single byte within `VmPageObjectState`.
    ///
    /// The byte is partitioned as:
    /// - bits 0..=4: `pin_count` (5 bits, max 31)
    /// - bit 5: `ALWAYS_NEED` (1 bit)
    /// - bits 6..=7: `dirty_state` (2 bits, max 3)
    #[repr(transparent)]
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    pub struct VmPageObjectFlags: u8 {
        /// Hint for whether the page is always needed and should not be considered for reclamation
        /// under memory pressure (unless the kernel decides to override hints for some reason).
        const ALWAYS_NEED = 1 << 5;
    }
}

impl VmPageObjectFlags {
    pub const PIN_COUNT_SHIFT: u32 = 0;
    pub const PIN_COUNT_MASK: u8 = ((1 << object::PIN_COUNT_BITS) - 1) as u8;

    pub const ALWAYS_NEED_SHIFT: u32 = 5;

    pub const DIRTY_STATE_SHIFT: u32 = 6;
    pub const DIRTY_STATE_MASK: u8 = (object::DIRTY_STATES_MASK as u8) << Self::DIRTY_STATE_SHIFT;

    /// Returns the pin count (0..=31).
    #[inline]
    pub const fn pin_count(self) -> u8 {
        (self.bits() >> Self::PIN_COUNT_SHIFT) & Self::PIN_COUNT_MASK
    }

    /// Sets the pin count, returning the updated flags.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `count > object::MAX_PIN_COUNT`.
    #[inline]
    pub fn with_pin_count(self, count: u8) -> Self {
        debug_assert!((count as u32) <= object::MAX_PIN_COUNT);
        let cleared = self.bits() & !Self::PIN_COUNT_MASK;
        Self::from_bits_retain(cleared | (count & Self::PIN_COUNT_MASK))
    }

    /// Returns whether the page is marked as always needed.
    #[inline]
    pub const fn always_need(self) -> bool {
        self.contains(Self::ALWAYS_NEED)
    }

    /// Sets or clears the `ALWAYS_NEED` flag.
    #[inline]
    pub fn with_always_need(mut self, needed: bool) -> Self {
        self.set(Self::ALWAYS_NEED, needed);
        self
    }

    /// Returns the raw dirty state value (0..=3).
    #[inline]
    pub const fn dirty_state(self) -> u8 {
        (self.bits() >> Self::DIRTY_STATE_SHIFT) & (object::DIRTY_STATES_MASK as u8)
    }

    /// Sets the dirty state, returning the updated flags.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `state >= object::MAX_DIRTY_STATES`.
    #[inline]
    pub fn with_dirty_state(self, state: u8) -> Self {
        debug_assert!((state as u32) < object::MAX_DIRTY_STATES);
        let cleared = self.bits() & !Self::DIRTY_STATE_MASK;
        let shifted = (state & (object::DIRTY_STATES_MASK as u8)) << Self::DIRTY_STATE_SHIFT;
        Self::from_bits_retain(cleared | shifted)
    }
}

zr::static_assert!(core::mem::size_of::<VmPageObjectFlags>() == 1);
zr::static_assert!(core::mem::align_of::<VmPageObjectFlags>() == 1);

/// Metadata stored in `VmPage` when the page is attached to a VM object.
#[repr(C, packed)]
#[derive(Default)]
pub struct VmPageObjectState {
    /// This is a back pointer to the vm object this page is currently contained in.  It is
    /// implicitly valid when the page is in a VmCowPages (which is a superset of intervals
    /// during which the page is in a page queue), and nullptr (or logically nullptr) otherwise.
    /// This should not be modified (except under the page queue lock) whilst a page is in a
    /// VmCowPages.
    /// Field should be modified by the setters and getters to allow for future encoding changes.
    pub object_priv: *mut core::ffi::c_void,
    /// When object_or_event_priv is pointing to a VmCowPages, this is the offset in the VmCowPages
    /// that contains this page.
    ///
    /// Else this field is 0.
    ///
    /// Field should be modified by the setters and getters to allow for future encoding changes.
    pub page_offset_priv: u64,
    /// Identifies how many objects can access this page when it doesn't have a unique owner. A
    /// page with a unique owner may still have multiple objects able to access it, just the count
    /// is not tracked.
    pub share_count: u32,
    /// Queue ID identifying which page queue this page is in.
    pub page_queue_priv: AtomicU8,
    /// Packed bitfield flags containing pin_count (5 bits), always_need (1 bit), dirty_state (2 bits).
    pub flags: VmPageObjectFlags,
}

impl VmPageObjectState {
    /// Returns the pin count (0..=31).
    #[inline]
    pub fn pin_count(&self) -> u8 {
        self.flags.pin_count()
    }

    /// Sets the pin count.
    #[inline]
    pub fn set_pin_count(&mut self, count: u8) {
        self.flags = self.flags.with_pin_count(count);
    }

    /// Returns whether the page is marked as always needed.
    #[inline]
    pub fn always_need(&self) -> bool {
        self.flags.always_need()
    }

    /// Sets the always_need flag.
    #[inline]
    pub fn set_always_need(&mut self, needed: bool) {
        self.flags = self.flags.with_always_need(needed);
    }

    /// Tracks state used to determine whether the page is dirty and its contents need to written
    /// back to the page source at some point, and when it has been cleaned. Used for pages backed
    /// by a user pager. The three states supported are Clean, Dirty, and AwaitingClean (more
    /// details in VmCowPages::DirtyState).
    #[inline]
    pub fn dirty_state(&self) -> u8 {
        self.flags.dirty_state()
    }

    /// Sets the dirty state.
    #[inline]
    pub fn set_dirty_state(&mut self, state: u8) {
        self.flags = self.flags.with_dirty_state(state);
    }
}

/// Metadata stored in `VmPage` for the ZRAM tri-page storage allocator.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct VmPageZramTriPage {
    // Tracks user-provided metadata for the item in each of the possible buckets.
    pub left_metadata: u32,
    pub mid_metadata: u32,
    pub right_metadata: u32,
    // Used by the VmTriPageStorage allocator to record the size of the item in each of the
    // possible buckets. See it for more details.
    pub left_compress_size: u16,
    pub mid_compress_size: u16,
    pub right_compress_size: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VmPageFreeState {
    pub _private: [u8; 0],
}

/// Used by the VmSlotPageStorage allocator to record free block information in the page.
/// See it for more details.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct VmPageZramSlot {
    pub free_block_mask: u64,
}

/// Metadata stored in `VmPage` when the page is in the ZRAM state.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub union VmPageZramState {
    pub tri_page: VmPageZramTriPage,
    pub slot: VmPageZramSlot,
}

/// Metadata stored in `VmPage` when the page is in the MMU state.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct VmPageMmuState {
    /// Optionally used by mmu code to count the number of valid mappings in this page if it is a
    /// page table.
    pub num_mappings: u32,
}

/// Metadata stored in `VmPage` when the page is in the ALLOC state.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct VmPageAllocState {
    /// Loaned pages maintain an optional backlink while in the alloc state to their holder object.
    pub owner: *mut core::ffi::c_void,
}

/// Metadata stored in `VmPage` when the page is in the SLAB state.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct VmPageSlabState {
    pub id: u32,
    pub free_slot: u32,
    pub peak_allocated: u32,
    pub allocated: u32,
    pub profile_cookie: u32,
}

/// Union of all possible state-dependent metadata in `VmPage`.
#[repr(C, packed)]
pub union VmPageUnion {
    // Pages in the free state have no extra state right now. This branch of the union is declared
    // in order to mark it as the default constructed one to match the default initial state.
    pub free: VmPageFreeState,
    pub object: core::mem::ManuallyDrop<VmPageObjectState>,
    pub zram: VmPageZramState,
    pub mmu: VmPageMmuState,
    pub alloc: VmPageAllocState,
    pub slab: VmPageSlabState,
}

impl Default for VmPageUnion {
    fn default() -> Self {
        Self { free: VmPageFreeState::default() }
    }
}

/// Core per-page bookkeeping structure allocated at PMM arena creation time.
///
/// # Page Ownership vs. Rust Ownership
///
/// In the Zircon VM subsystem, there is a fundamental distinction between
/// **conceptual ownership of the page** (the physical memory allocation and its
/// lifecycle) and **Rust ownership of the `VmPage` data structure**:
///
/// - **Conceptual Page Ownership**: Subsystems such as `PmmNode`, `VmCowPages`,
///   or page queues obtain conceptual ownership over a page when allocating,
///   managing, or transferring it. This conceptual ownership governs the page's
///   physical memory allocation and lifecycle (e.g. committing, pinning, loaning,
///   or freeing the physical frame). Conceptual ownership also confers ownership
///   over *specific subfields* of the `VmPage` at specific phases of the page's
///   lifecycle. For example:
///   - When a page is held in a `VmCowPages`, that VM container possesses
///     ownership of the active union subfields (`object`, `page_offset`) in
///     `state_union`, subject to synchronization rules (e.g. modifications
///     occurring under the page queue lock).
///   - When a page is in a page queue or a PMM free list, the queue or list
///     owns the `queue_node` subfield for intrusive list linkage.
///   - Transitioning the page lifecycle state (`set_state`) requires conceptual
///     ownership of the page or holding the relevant subsystem lock.
///
/// - **Rust Ownership of the `VmPage` Data Structure**: All `VmPage` instances
///   are statically allocated at boot time as contiguous arrays of bookkeeping
///   memory inside PMM arenas. They remain permanently resident in memory and
///   are referenced by physical addresses, arenas, page queues, and raw pointers
///   across both C++ and Rust. Consequently, having conceptual ownership of a
///   page and the physical allocation it represents does **not** grant full,
///   exclusive Rust ownership (`VmPage` value ownership or `&mut VmPage`) over
///   the `VmPage` data structure. Multiple subsystems or threads may concurrently
///   hold shared references (`&VmPage`) or perform atomic queries on fields such
///   as `state_priv` and `loaned_state_priv`.
///
/// Because exclusive Rust ownership over `VmPage` can almost never be soundly
/// obtained, `&mut VmPage` or mutable borrows are fundamentally inappropriate.
/// Instead:
/// - Subfields that are mutated under subfield-level ownership or lock protection
///   (`queue_node` and `state_union`) are wrapped in [`UnsafeCell`].
/// - Fields subject to concurrent or lock-free inspection use atomics ([`AtomicU8`]).
/// - Fields that are immutable after arena initialization remain regular values
///   ([`usize`] for `paddr_priv`).
/// - All methods on `VmPage` take a shared reference (`&self`), requiring callers
///   to satisfy safety preconditions (holding conceptual ownership of the relevant
///   subfield or the appropriate subsystem locks) to access or mutate interior
///   state.
#[repr(C)]
pub struct VmPage {
    /// Intrusive doubly linked list node for page queues or free lists.
    ///
    /// Wrapped in an [`UnsafeCell`] because ownership of this linkage node is
    /// transferred between page queues and free lists while the underlying
    /// `VmPage` is shared.
    pub queue_node: UnsafeCell<fbl::DoublyLinkedListNode<VmPage>>,

    /// Physical address of the page (read-only after setup).
    pub paddr_priv: usize,

    /// State-dependent metadata union.
    ///
    /// Wrapped in an [`UnsafeCell`] because conceptual ownership of the page
    /// grants ownership over specific active union variants/fields without
    /// granting exclusive Rust ownership of the `VmPage` struct.
    pub state_union: UnsafeCell<VmPageUnion>,

    /// logically private; use |state()| and |set_state()|
    pub state_priv: AtomicU8,

    /// logically private, use loaned getters and setters below.
    /// The loaned state is packed into a single byte here to reduce memory usage, but due to the
    /// allowable access patterns this means the getters and setters must perform atomic loads and
    /// stores to prevent UB.
    pub loaned_state_priv: AtomicU8,
}

// Compile-time layout assertions matching C++ vm_page_t via bindgen.
zr::static_assert!(core::mem::size_of::<VmPage>() == core::mem::size_of::<bindings::vm_page_t>());
zr::static_assert!(core::mem::align_of::<VmPage>() == core::mem::align_of::<bindings::vm_page_t>());
zr::static_assert!(core::mem::size_of::<VmPage>() == 48);
zr::static_assert!(core::mem::align_of::<VmPage>() == 8);
zr::static_assert!(core::mem::offset_of!(VmPage, queue_node) == 0);
zr::static_assert!(core::mem::offset_of!(VmPage, paddr_priv) == 16);
zr::static_assert!(core::mem::offset_of!(VmPage, state_union) == 24);
zr::static_assert!(core::mem::offset_of!(VmPage, state_priv) == 46);
zr::static_assert!(core::mem::offset_of!(VmPage, loaned_state_priv) == 47);

impl VmPage {
    pub const LOANED_STATE_IS_LOANED: u8 = 1;
    pub const LOANED_STATE_IS_LOAN_CANCELLED: u8 = 2;

    /// Returns whether this page is in the FREE state. When in the FREE state the page is assumed to
    /// be conceptually owned by the relevant PmmNode, and hence unless its lock is held this query
    /// must be assumed to be racy.
    pub fn is_free(&self) -> bool {
        self.state() == VmPageState(bindings::vm_page_state::FREE)
    }

    /// Returns whether this page is in the FREE_LOANED state. Similar to the FREE state the page is
    /// assumed to be conceptually owned by the relevant PmmNode, however this distinguishes whether the
    /// page is part of the general purpose free list, versus the more narrowly usable set of loaned pages.
    pub fn is_free_loaned(&self) -> bool {
        self.state() == VmPageState(bindings::vm_page_state::FREE_LOANED)
    }

    /// If true, this page is "loaned" in the sense of being loaned from a contiguous VMO (via
    /// decommit) to Zircon.  If the original contiguous VMO is deleted, this page will no longer be
    /// loaned.  A loaned page cannot be pinned.  Instead a different physical page (non-loaned) is
    /// used for the pin.  A loaned page can be (re-)committed back into its original contiguous VMO,
    /// which causes the data in the loaned page to be moved into a different physical page (which
    /// itself can be non-loaned or loaned).  A loaned page cannot be used to allocate a new contiguous
    /// VMO.
    /// May be queried by anyone who either has conceptual ownership of the page, or has sufficient
    /// knowledge that the loaned state cannot be being altered in parallel.
    pub fn is_loaned(&self) -> bool {
        (self.loaned_state_priv.load(Ordering::Relaxed) & Self::LOANED_STATE_IS_LOANED) != 0
    }

    /// If true, the original contiguous VMO wants the page back.  Such pages won't be reused until
    /// the page is no longer loaned, either via commit of the page back into the contiguous VMO that
    /// loaned the page, or via deletion of the contiguous VMO that loaned the page.  Such pages are
    /// not in the free_loaned_list_ in pmm, which is how reuse is prevented.
    /// Should only be called by the PmmNode under its lock.
    pub fn is_loan_cancelled(&self) -> bool {
        (self.loaned_state_priv.load(Ordering::Relaxed) & Self::LOANED_STATE_IS_LOAN_CANCELLED) != 0
    }

    /// Sets the loaned flag on the page.
    /// Manipulation of 'loaned' should only be done by the PmmNode under the loaned pages lock whilst
    /// it has conceptual ownership of the page.
    pub fn set_is_loaned(&self) {
        self.loaned_state_priv.fetch_or(Self::LOANED_STATE_IS_LOANED, Ordering::Relaxed);
    }

    /// Clears the loaned flag on the page.
    /// Manipulation of 'loaned' should only be done by the PmmNode under the loaned pages lock whilst
    /// it has conceptual ownership of the page.
    pub fn clear_is_loaned(&self) {
        self.loaned_state_priv.fetch_and(!Self::LOANED_STATE_IS_LOANED, Ordering::Relaxed);
    }

    /// Sets the loan_cancelled flag on the page.
    /// Manipulation of 'loan_cancelled' should only be done by the PmmNode under its lock, but may be
    /// done when the PmmNode does not have conceptual ownership of the page.
    pub fn set_is_loan_cancelled(&self) {
        self.loaned_state_priv.fetch_or(Self::LOANED_STATE_IS_LOAN_CANCELLED, Ordering::Relaxed);
    }

    /// Clears the loan_cancelled flag on the page.
    /// Manipulation of 'loan_cancelled' should only be done by the PmmNode under its lock, but may be
    /// done when the PmmNode does not have conceptual ownership of the page.
    pub fn clear_is_loan_cancelled(&self) {
        self.loaned_state_priv.fetch_and(!Self::LOANED_STATE_IS_LOAN_CANCELLED, Ordering::Relaxed);
    }

    /// Dumps information about the page to the debuglog.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it is safe to inspect the page state and union metadata
    /// (e.g. by holding conceptual ownership of the page or the appropriate subsystem locks).
    pub unsafe fn dump(&self) {
        let page_state = self.state();
        kprint::kprint!(
            "page {:p}: address {:#x} state {:s}",
            self as *const VmPage,
            self.paddr().0,
            super::page_state::page_state_to_string(page_state)
        );
        if page_state == VmPageState(bindings::vm_page_state::OBJECT) {
            // SAFETY: Dereferencing UnsafeCell to read the object field from the active union variant.
            // The caller guarantees `object` is active and that reading it does not race with concurrent writes.
            let object = unsafe { &*(*self.state_union.get()).object };
            kprint::kprintln!(
                " pin_count {:d} share_count {:u}",
                object.pin_count() as i32,
                object.share_count
            );
        } else {
            kprint::kprintln!("");
        }
    }

    /// Return the physical address of the page.
    // future plan to store in a compressed form
    pub const fn paddr(&self) -> PAddr {
        PAddr(self.paddr_priv)
    }

    /// Return the current VmPageState of this page.
    pub fn state(&self) -> VmPageState {
        // SAFETY: state_priv is only set to valid vm_page_state values.
        VmPageState(unsafe {
            core::mem::transmute::<u8, page_bindings::vm_page_state>(
                self.state_priv.load(Ordering::Relaxed),
            )
        })
    }

    /// Sets the VmPageState of this page. Although the implementation of this method is just
    /// modifying an atomic, the state value is assumed to be 'correct' elsewhere in the code and
    /// so incorrect modifications will result in memory safety errors.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it owns the page or holds the necessary locks to modify its
    /// state.
    pub unsafe fn set_state(&self, new_state: VmPageState) {
        let old_state = self.state();
        self.state_priv.store(new_state.as_raw(), Ordering::Relaxed);

        // See comment at percpu::vm_page_counts
        let p = PerCpu::get_current();
        p.vm_page_counts.by_state[old_state.index()].fetch_sub(1);
        p.vm_page_counts.by_state[new_state.index()].fetch_add(1);
    }

    /// Returns the backlink object pointer for the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `object` is the currently active union variant, and that reading
    /// this subfield is safe (e.g. by possessing conceptual ownership of this subfield or holding
    /// the page queue lock).
    pub unsafe fn get_object(&self) -> *mut core::ffi::c_void {
        // SAFETY: Dereferencing UnsafeCell to read the object field from the active union variant.
        // The caller guarantees `object` is active and that reading it does not race with concurrent writes.
        unsafe { (*(*self.state_union.get()).object).object_priv }
    }

    /// Sets the backlink object pointer for the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `object` is the currently active union variant, and that it has
    /// ownership of this subfield or holds the page queue lock to modify it safely without data races.
    pub unsafe fn set_object(&self, object: *mut core::ffi::c_void) {
        // SAFETY: Dereferencing UnsafeCell to mutate the object field in the active union variant.
        // The caller guarantees ownership of this subfield or holding the page queue lock.
        unsafe { (*(*self.state_union.get()).object).object_priv = object };
    }

    /// Returns the page offset in the backlink object for the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `object` is the currently active union variant, and that reading
    /// this subfield is safe (e.g. by possessing conceptual ownership of this subfield or holding
    /// the page queue lock).
    pub unsafe fn get_page_offset(&self) -> u64 {
        // SAFETY: Dereferencing UnsafeCell to read the page_offset field from the active union variant.
        // The caller guarantees `object` is active and that reading it does not race with concurrent writes.
        unsafe { (*(*self.state_union.get()).object).page_offset_priv }
    }

    /// Sets the page offset in the backlink object for the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `object` is the currently active union variant, and that it has
    /// ownership of this subfield or holds the page queue lock to modify it safely without data races.
    pub unsafe fn set_page_offset(&self, offset: u64) {
        // SAFETY: Dereferencing UnsafeCell to mutate the page_offset field in the active union variant.
        // The caller guarantees ownership of this subfield or holding the page queue lock.
        unsafe { (*(*self.state_union.get()).object).page_offset_priv = offset };
    }

    /// Gets the pin count.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `object` is the currently active union variant, and that reading
    /// this subfield is safe without data races.
    pub unsafe fn get_pin_count(&self) -> u8 {
        // SAFETY: Dereferencing UnsafeCell to read the object flags from the active union variant.
        // The caller guarantees `object` is active and that reading it does not race with concurrent writes.
        unsafe { (*(*self.state_union.get()).object).pin_count() }
    }

    /// Get a reference to the page queue atomic.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `object` is the currently active union variant.
    pub unsafe fn get_page_queue_ref(&self) -> &AtomicU8 {
        // SAFETY: Dereferencing UnsafeCell to obtain a shared reference to the AtomicU8 in the
        // active union variant. Atomic operations on the reference are thread-safe.
        unsafe { &(*(*self.state_union.get()).object).page_queue_priv }
    }
}

impl Default for VmPage {
    fn default() -> Self {
        Self {
            queue_node: UnsafeCell::new(fbl::DoublyLinkedListNode::new()),
            paddr_priv: 0,
            state_union: UnsafeCell::new(VmPageUnion::default()),
            state_priv: AtomicU8::new(bindings::vm_page_state::FREE as u8),
            loaned_state_priv: AtomicU8::new(0),
        }
    }
}

impl fbl::DoublyLinkedListContainable<VmPage> for VmPage {
    fn get_node(&self) -> &fbl::DoublyLinkedListNode<VmPage> {
        // SAFETY: `queue_node` is an UnsafeCell wrapping `DoublyLinkedListNode`. Returning a
        // shared reference to the node is safe because the node manages its own internal
        // mutability via UnsafeCells or list synchronization.
        unsafe { &*self.queue_node.get() }
    }
}

/// Type-safe wrapper around a raw pointer to a kernel page.
///
/// A `VmPagePtr` represents a pointer to a `VmPage` whose lifetime is managed by PMM arenas.
/// Possessing a `VmPagePtr` does NOT grant exclusive Rust ownership of the underlying `VmPage`,
/// and therefore `VmPagePtr` does not provide mutable references (`&mut VmPage`). Instead,
/// operations that mutate page metadata require the caller to possess conceptual ownership of the
/// page or holding the relevant subsystem locks, operating through interior mutability.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmPagePtr(NonNull<VmPage>);

impl VmPagePtr {
    /// Construct a `VmPagePtr` from an existing `NonNull<VmPage>`
    pub fn new(ptr: NonNull<VmPage>) -> Self {
        VmPagePtr(ptr)
    }

    /// Creates a `VmPagePtr` from a raw pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a kernel page.
    pub const unsafe fn from_raw(ptr: *mut VmPage) -> Option<Self> {
        match NonNull::new(ptr) {
            Some(nn) => Some(Self(nn)),
            None => None,
        }
    }

    /// Returns the non-null pointer.
    pub fn as_non_null(self) -> NonNull<VmPage> {
        self.0
    }

    /// Returns the raw pointer.
    pub fn as_raw(self) -> *mut VmPage {
        self.0.as_ptr()
    }

    /// Temporary helper for interacting with FFI methods that, due to how the bindings are auto
    /// generated, expect a *vm_page_t and not a *VmPage.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a kernel page.
    pub unsafe fn from_ffi(ptr: *mut bindings::vm_page_t) -> Option<Self> {
        // SAFETY: Method preconditions match from_raw requirements.
        unsafe { Self::from_raw(ptr.cast()) }
    }

    /// Temporary helper for interacting with FFI methods that, due to how the bindings are auto
    /// generated, expect a *vm_page_t and not a *VmPage.
    pub fn as_ffi(self) -> *mut bindings::vm_page_t {
        self.0.as_ptr().cast()
    }

    /// Return a reference to the underlying `VmPage`
    ///
    /// # Safety
    ///
    /// The caller must ensure that `self` points to a valid, live `VmPage`. Note that obtaining
    /// a shared `&VmPage` does NOT imply exclusive Rust ownership over the page structure or its
    /// subfields; callers must respect the safety requirements of individual subfields and methods.
    pub unsafe fn as_ref(&self) -> &VmPage {
        // SAFETY: The caller guarantees that the pointer is valid and properly aligned.
        unsafe { self.0.as_ref() }
    }

    /// Returns whether this page is in the FREE state. When in the FREE state the page is assumed
    /// to be conceptually owned by the relevant PmmNode, and hence unless its lock is held this query
    /// must be assumed to be racy.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it either has conceptual ownership of the page or knows it is safe to
    /// inspect the state.
    pub unsafe fn is_free(self) -> bool {
        // SAFETY: The caller guarantees via function safety preconditions that it is safe to
        // inspect the page state.
        unsafe { self.as_ref().is_free() }
    }

    /// Returns whether this page is in the FREE_LOANED state. Similar to the FREE state the page is
    /// assumed to be conceptually owned by the relevant PmmNode, however this distinguishes whether
    /// the page is part of the general purpose free list, versus the more narrowly usable set of loaned pages.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it either has conceptual ownership of the page or knows it is safe to
    /// inspect the state.
    pub unsafe fn is_free_loaned(self) -> bool {
        // SAFETY: The caller guarantees via function safety preconditions that it is safe to
        // inspect the page state.
        unsafe { self.as_ref().is_free_loaned() }
    }

    /// If true, this page is "loaned" in the sense of being loaned from a contiguous VMO (via
    /// decommit) to Zircon.  If the original contiguous VMO is deleted, this page will no longer be
    /// loaned.  A loaned page cannot be pinned.  Instead a different physical page (non-loaned) is
    /// used for the pin.  A loaned page can be (re-)committed back into its original contiguous
    /// VMO, which causes the data in the loaned page to be moved into a different physical page
    /// (which itself can be non-loaned or loaned).  A loaned page cannot be used to allocate a new
    /// contiguous VMO. May be queried by anyone who either has conceptual ownership of the page, or has
    /// sufficient knowledge that the loaned state cannot be being altered in parallel.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it either has conceptual ownership of the page or knows it is safe to
    /// inspect the state.
    pub unsafe fn is_loaned(self) -> bool {
        // SAFETY: The caller guarantees via function safety preconditions that it is safe to
        // inspect the loaned state.
        unsafe { self.as_ref().is_loaned() }
    }

    /// If true, the original contiguous VMO wants the page back.  Such pages won't be reused until
    /// the page is no longer loaned, either via commit of the page back into the contiguous VMO
    /// that loaned the page, or via deletion of the contiguous VMO that loaned the page. Such pages
    /// are not in the free_loaned_list_ in pmm, which is how reuse is prevented. Should only be
    /// called by the PmmNode under its lock.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it either has conceptual ownership of the page or knows it is safe to
    /// inspect the state.
    pub unsafe fn is_loan_cancelled(self) -> bool {
        // SAFETY: The caller guarantees via function safety preconditions that it is safe to
        // inspect the loaned state.
        unsafe { self.as_ref().is_loan_cancelled() }
    }

    /// Sets the loaned flag on the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it has conceptual ownership of the page and holds the loaned pages
    /// lock of the PmmNode.
    pub unsafe fn set_is_loaned(self) {
        // SAFETY: The caller guarantees conceptual ownership of the page and holds the necessary PmmNode lock.
        unsafe { self.as_ref().set_is_loaned() }
    }

    /// Clears the loaned flag on the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it has conceptual ownership of the page and holds the loaned pages
    /// lock of the PmmNode.
    pub unsafe fn clear_is_loaned(self) {
        // SAFETY: The caller guarantees conceptual ownership of the page and holds the necessary PmmNode lock.
        unsafe { self.as_ref().clear_is_loaned() }
    }

    /// Sets the loan_cancelled flag on the page. May be done even if not the conceptual owner of the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it holds the loaned pages lock of the PmmNode.
    pub unsafe fn set_is_loan_cancelled(self) {
        // SAFETY: The caller guarantees holding the necessary PmmNode lock.
        unsafe { self.as_ref().set_is_loan_cancelled() }
    }

    /// Clears the loan_cancelled flag on the page. May be done even if not the conceptual owner of the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it holds the loaned pages lock of the PmmNode.
    pub unsafe fn clear_is_loan_cancelled(self) {
        // SAFETY: The caller guarantees holding the necessary PmmNode lock.
        unsafe { self.as_ref().clear_is_loan_cancelled() }
    }

    /// Dumps information about the page to the debuglog.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it is safe to inspect the page state and union metadata (e.g. by
    /// holding conceptual ownership of the page or the appropriate subsystem locks).
    pub unsafe fn dump(self) {
        // SAFETY: The caller guarantees via function safety preconditions that it is safe to access
        // the page state.
        unsafe { self.as_ref().dump() }
    }

    /// Return the physical address of the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it either has conceptual ownership of the page or knows it is safe to
    /// inspect the state.
    pub unsafe fn paddr(self) -> PAddr {
        // SAFETY: The caller guarantees via function safety preconditions that it is safe to
        // inspect the page state.
        unsafe { self.as_ref().paddr() }
    }

    /// Returns the backlink object pointer for the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the page is attached to a VM object (`state_union` is in `object` variant)
    /// and that reading this subfield is safe without data races.
    pub unsafe fn get_object(self) -> *mut core::ffi::c_void {
        // SAFETY: Safety deferred to caller per function safety preconditions.
        unsafe { self.as_ref().get_object() }
    }

    /// Returns the page offset in the backlink object for the page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the page is attached to a VM object (`state_union` is in `object` variant)
    /// and that reading this subfield is safe without data races.
    pub unsafe fn get_page_offset(self) -> u64 {
        // SAFETY: Safety deferred to caller per function safety preconditions.
        unsafe { self.as_ref().get_page_offset() }
    }

    /// Returns the pin count of the page when attached to a VM object.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the page is attached to a VM object (`state_union` is in `object` variant)
    /// and that reading this subfield is safe without data races.
    pub unsafe fn get_pin_count(self) -> u8 {
        // SAFETY: Safety deferred to caller per function safety preconditions.
        unsafe { self.as_ref().get_pin_count() }
    }

    /// Return the current VmPageState of this page.
    pub fn state(self) -> VmPageState {
        unsafe { self.as_ref().state() }
    }

    /// Sets the VmPageState of this page.
    ///
    /// # Safety
    ///
    /// The caller must ensure that it has conceptual ownership of the page (dictating its lifecycle)
    /// or holds the necessary locks to modify its state.
    pub unsafe fn set_state(self, new_state: VmPageState) {
        // SAFETY: The caller guarantees conceptual ownership of the page or holding the necessary
        // locks to modify its state.
        unsafe { self.as_ref().set_state(new_state) }
    }
}

pub type VmPageDoublyLinkedList = fbl::DoublyLinkedList<NonNull<VmPage>>;

/// Return the approximate number of pages in state |state|.
///
/// When called concurrently with |set_state|, the count may be off by a small amount.
pub fn get_count(state: VmPageState) -> u64 {
    let mut result: i64 = 0;
    PerCpu::for_each_preempt_disable(|_cpu, p| {
        // Because `get_count` could be called concurrently with `set_state` we're not guaranteed to
        // get a consistent snapshot of the page counts. It's OK if the values are a little off. See
        // comment at the definition of `vm_page_state`.
        result += p.vm_page_counts.by_state[state.index()].load();
    });
    if result >= 0 { result as u64 } else { 0 }
}

/// Add |n| to the count of pages in state |state|.
///
/// Should be used when first constructing pages.
pub fn add_to_initial_count(state: VmPageState, n: u64) {
    PerCpu::with_current_preempt_disable(|p| {
        p.vm_page_counts.by_state[state.index()].fetch_add(n as i64);
    })
}

// FFI trampolines exported to C++

/// FFI trampoline for dumping information about `page` from C++.
///
/// # Safety
///
/// The caller must ensure that `page` points to a valid `VmPage` and that it is safe to inspect
/// the page state and union metadata.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_vm_page_dump(page: &VmPage) {
    // SAFETY: Preconditions forwarded from caller.
    unsafe {
        page.dump();
    }
}

/// FFI trampoline for getting the approximate number of pages in `state` from C++.
#[unsafe(no_mangle)]
pub extern "C" fn rust_vm_page_get_count(state: bindings::vm_page_state) -> u64 {
    get_count(VmPageState(state))
}

/// FFI trampoline for adding `n` to the count of pages in `state` from C++.
#[unsafe(no_mangle)]
pub extern "C" fn rust_vm_page_add_to_initial_count(state: bindings::vm_page_state, n: u64) {
    add_to_initial_count(VmPageState(state), n);
}

#[cfg(console_enabled)]
/// Console command handler for `vm_page`.
///
/// # Safety
///
/// If `argc > 0` and `argv` is non-null, `argv` must point to `argc` valid `CmdArgs` structures.
unsafe extern "C" fn cmd_vm_page(
    argc: core::ffi::c_int,
    argv: *const CmdArgs,
    _flags: u32,
) -> core::ffi::c_int {
    // SAFETY: The console framework guarantees argv points to argc elements if argv is non-null.
    let args = if argc > 0 && !argv.is_null() {
        unsafe { core::slice::from_raw_parts(argv, argc as usize) }
    } else {
        &[]
    };

    let cmd_name = if !args.is_empty() {
        match args[0].as_str() {
            "" => "vm_page",
            s => s,
        }
    } else {
        "vm_page"
    };

    let usage = || -> core::ffi::c_int {
        kprint::kprint!("usage:\n");
        kprint::kprint!("{:s} dump <address>\n", cmd_name);
        kprint::kprint!("{:s} hexdump <address>\n", cmd_name);
        zx_status::Status::INTERNAL.into_raw()
    };

    let not_enough_args = || -> core::ffi::c_int {
        kprint::kprint!("not enough arguments\n");
        usage()
    };

    if argc < 2 || argv.is_null() {
        return not_enough_args();
    }

    match args[1].as_str() {
        "dump" => {
            if argc < 3 {
                return not_enough_args();
            }

            let page = args[2].arg_uint as *const VmPage;
            if page.is_null() {
                kprint::kprint!("bad page or page not mapped in kernel space\n");
                return zx_status::Status::INTERNAL.into_raw();
            }

            // SAFETY: The caller provides the address of a VmPage to dump.
            unsafe {
                (*page).dump();
            }
        }
        "hexdump" => {
            if argc < 3 {
                return not_enough_args();
            }

            let page = args[2].arg_uint as *const VmPage;
            if page.is_null() {
                kprint::kprint!("bad page or page not mapped in kernel space\n");
                return zx_status::Status::INTERNAL.into_raw();
            }

            // SAFETY: Caller passed address of a page.
            let pa = unsafe { (*page).paddr() };
            let vaddr = super::physmap::paddr_to_physmap(pa);
            if vaddr.0 == 0 {
                kprint::kprint!("bad page or page not mapped in kernel space\n");
                return zx_status::Status::INTERNAL.into_raw();
            }

            let mut writer = debug::dprintf::KernelConsoleWriter;
            // SAFETY: `vaddr` points to a valid mapped page in physmap of size `::page::SIZE`.
            let _ = unsafe {
                pretty::hexdump_very_ex_raw(
                    &mut writer,
                    vaddr.0 as *const u8,
                    ::page::SIZE,
                    vaddr.0 as u64,
                )
            };
        }
        _ => {
            kprint::kprint!("unknown command\n");
            return usage();
        }
    }

    zx_status::sys::ZX_OK
}

#[cfg(console_enabled)]
static_command!(
    CMD_VM_PAGE,
    c"vm_page".as_ptr(),
    c"vm_page debug commands".as_ptr(),
    cmd_vm_page,
    CMD_AVAIL_NORMAL
);
