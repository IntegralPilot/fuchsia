// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::pmm::node as pmm_node;
use crate::kernel::types::PAddr;
use core::ops::Deref;
use core::pin::Pin;
use debug::{ltracef, ltracef_level};
use pin_init::pin_data;
use vm_constants_rs::{
    kPmmNodeIndexZeroBits, kVmPageListFanOut, kVmPageListIntervalBits,
    kVmPageListIntervalSentinelBits, kVmPageListIntervalType, kVmPageListIntervalTypeBits,
    kVmPageListPageType, kVmPageListParentContentType, kVmPageListReferenceType,
    kVmPageListTypeBits, kVmPageListZeroMarkerType,
};
use vm_page_list_bindings as bindings;
use zr::{Opaque, pin_init_ffi, unsafe_pinned_drop_ffi};
use zx_status::Status;

use crate::vm::page::VmPagePtr;

const LOCAL_TRACE: u32 = 0;

/// RAII helper for representing content in a page list node. This supports being in one of these
/// states:
///  * Empty       - Contains nothing.
///  * Page p      - Contains a `VmPagePtr` 'p'. This 'p' is considered owned by this wrapper and
///    `release_page` must be called to give up ownership.
///  * Reference r - Contains a reference 'r' to some content. This 'r' is considered owned by this
///    wrapper and `release_reference` must be called to give up ownership.
///  * Marker      - Indicates that whilst not a page, it is also not empty. Markers can be used to
///    separate the distinction between "there's no page because we've deduped to the zero page" (a
///    `Marker` is inserted) and "there's no page because our parent contains the content" (which is
///    represented as `Empty`).
///  * ParentContent - Indicates that there might be content for this slot, but the page list in the
///    parent must be checked for it. The difference between `Empty`, which can also indicate that
///    the parent must be searched, and `ParentContent` is up to the specific VMO.
///  * Interval    - Indicates that this page is part of a sparse page interval. An interval will
///    have a Start sentinel, and an End sentinel, and all offsets that lie between the two will be
///    empty. If the interval spans a single page, it will be represented as a Slot sentinel, which
///    is conceptually the same as both a Start and an End sentinel.
///
/// There are certain invariants that the page list tries to maintain at all times. It might not
/// always be possible to enforce these as the checks involved might be expensive, however it is
/// important that any code that manipulates the page list abide by them, primarily to keep the
/// memory occupied by the page list nodes in check:
/// 1. Page list nodes cannot be completely empty i.e. they must contain at least one non-empty
///    slot.
/// 2. Any intervals in the page list should span a maximal range. In other words, there should not
///    be consecutive intervals in the page list which it would have been possible to represent with
///    a single interval instead.
///
/// Note on Preconditions & Safety Contracts:
/// `VmPageOrMarker` uses manual bit-packing rather than a native Rust `enum` to maintain
/// C++ memory layout parity and zero-overhead performance. Accessors use `debug_assert!`
/// to validate variant preconditions in debug builds, matching C++ `DEBUG_ASSERT` behavior.
#[derive(Debug)]
#[repr(transparent)]
pub struct VmPageOrMarker {
    raw: u32,
}

/// The types of sparse page interval types that are supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum IntervalType {
    /// Represents a range of zero pages.
    Zero = 0,
}

/// Sentinel types that are used to represent a sparse page interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum SentinelType {
    /// Represents a single page interval.
    Slot = 0,
    /// The first page of a multi-page interval.
    Start,
    /// The last page of a multi-page interval.
    End,
}

/// The various dirty states that a zero interval can be in. Refer to VmCowPages::DirtyState for
/// an explanation of the states. Note that an AwaitingClean state is not encoded in the interval
/// state bits. This information is instead stored using the awaiting_clean_length for convenience,
/// where a non-zero length indicates that the interval is AwaitingClean. Doing this affords
/// more convenient splitting and merging of intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ZeroRangeDirtyState {
    /// Dirty state is untracked.
    Untracked = 0,
    /// Range is clean.
    Clean,
    /// Range is dirty.
    Dirty,
}

/// The remaining bits of an interval type store any information specific to the type of interval
/// being tracked. The ZeroRange class is defined here to group together the encoding of these bits
/// specific to IntervalType::Zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ZeroRange {
    value: u32,
}

impl ZeroRange {
    // This is the same as kIntervalBits in C++.
    const ALIGN_BITS: u32 = kVmPageListIntervalBits;
    const ALIGN_MASK: u32 = (1 << Self::ALIGN_BITS) - 1;
    // Bound to VM_PAGE_OBJECT_DIRTY_STATE_BITS in C++.
    // For zero range tracking, we also need to track dirty state information, and if the interval
    // is AwaitingClean, the length that is AwaitingClean.
    const DIRTY_STATE_BITS: u32 = crate::vm::page::object::DIRTY_STATE_BITS;
    const DIRTY_STATE_SHIFT: u32 = Self::ALIGN_BITS;
    const AWAITING_CLEAN_LENGTH_SHIFT: u32 = Self::ALIGN_BITS + Self::DIRTY_STATE_BITS;

    const PAGE_SHIFT: u32 = 12;
    const PAGE_SIZE: u64 = 1 << Self::PAGE_SHIFT;
    const DIRTY_STATE_MASK: u32 = ((1 << Self::DIRTY_STATE_BITS) - 1) << Self::DIRTY_STATE_SHIFT;
    const AWAITING_CLEAN_LENGTH_MASK: u32 = !((1 << Self::AWAITING_CLEAN_LENGTH_SHIFT) - 1);

    /// Creates a new `ZeroRange` with a raw aligned value.
    const fn new(val: u32) -> Self {
        debug_assert!((val & Self::ALIGN_MASK) == 0);
        Self { value: val }
    }

    /// Creates a `ZeroRange` with a raw aligned value and initial dirty state.
    fn new_with_state(val: u32, state: ZeroRangeDirtyState) -> Self {
        let mut this = Self::new(val);
        this.set_dirty_state(state);
        this
    }

    /// Returns the raw packed `u32` value.
    const fn value(&self) -> u32 {
        self.value
    }

    /// Returns the `ZeroRangeDirtyState`.
    fn dirty_state(&self) -> ZeroRangeDirtyState {
        let state_bits = (self.value & Self::DIRTY_STATE_MASK) >> Self::DIRTY_STATE_SHIFT;
        match state_bits {
            0 => ZeroRangeDirtyState::Untracked,
            1 => ZeroRangeDirtyState::Clean,
            2 => ZeroRangeDirtyState::Dirty,
            _ => panic!("invalid dirty state"),
        }
    }

    /// Sets the `ZeroRangeDirtyState`.
    fn set_dirty_state(&mut self, state: ZeroRangeDirtyState) {
        debug_assert!(
            matches!(state, ZeroRangeDirtyState::Dirty | ZeroRangeDirtyState::Untracked),
            "Only allow dirty and untracked zero ranges for now"
        );
        self.value =
            (self.value & !Self::DIRTY_STATE_MASK) | ((state as u32) << Self::DIRTY_STATE_SHIFT);
    }

    /// Sets the page-aligned awaiting clean length.
    fn set_awaiting_clean_length(&mut self, len: u64) {
        debug_assert!(len == 0 || self.dirty_state() == ZeroRangeDirtyState::Dirty);
        debug_assert!(len.is_multiple_of(Self::PAGE_SIZE), "len must be page-aligned");
        // The awaiting clean length is always page-aligned, so mask out the low bits and store only
        // upper bits.
        let encoded_len = ((len >> Self::PAGE_SHIFT) << Self::AWAITING_CLEAN_LENGTH_SHIFT) as u32;
        self.value = (self.value & !Self::AWAITING_CLEAN_LENGTH_MASK)
            | (encoded_len & Self::AWAITING_CLEAN_LENGTH_MASK);
    }

    /// Returns the page-aligned awaiting clean length.
    fn awaiting_clean_length(&self) -> u64 {
        let len_bits = (self.value & Self::AWAITING_CLEAN_LENGTH_MASK) as u64;
        (len_bits >> Self::AWAITING_CLEAN_LENGTH_SHIFT) << Self::PAGE_SHIFT
    }
}

/// Minimal wrapper around a `u32` to provide stronger typing in code to prevent accidental
/// mixing of references and other values.
/// Provides a way to query the required alignment of the references and does debug enforcement of
/// this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct ReferenceValue {
    value: u32,
}

impl ReferenceValue {
    // ALIGN_BITS represents the number of low bits in a reference that must be zero so they can be
    // used for internal metadata. This is declared here for convenience, and is asserted to be in
    // sync with the private VmPageOrMarker::TYPE_BITS.
    pub const ALIGN_BITS: u32 = 3;
    pub const ALIGN_MASK: u32 = (1 << Self::ALIGN_BITS) - 1;

    /// Creates a new `ReferenceValue`.
    pub const fn new(val: u32) -> Self {
        debug_assert!((val & Self::ALIGN_MASK) == 0);
        Self { value: val }
    }

    /// Returns the raw value.
    pub fn value(&self) -> u32 {
        self.value
    }
}

impl VmPageOrMarker {
    // The low 3 bits of raw are reserved to represent the type, any other data has to fit into
    // the remaining high bits. Note that there is no explicit Empty type, rather a PAGE_TYPE with a
    // zero pointer is used to represent Empty.
    const TYPE_BITS: u32 = kVmPageListTypeBits;
    const TYPE_MASK: u32 = (1 << Self::TYPE_BITS) - 1;
    const PAGE_TYPE: u32 = kVmPageListPageType;
    const ZERO_MARKER_TYPE: u32 = kVmPageListZeroMarkerType;
    const REFERENCE_TYPE: u32 = kVmPageListReferenceType;
    const INTERVAL_TYPE: u32 = kVmPageListIntervalType;
    const PARENT_CONTENT_TYPE: u32 = kVmPageListParentContentType;

    const MARKER_SHARE_COUNT_MASK: u32 = !Self::TYPE_MASK;
    const MARKER_SHARE_COUNT_STEP: u32 = 1 << Self::TYPE_BITS;

    // In addition to storing the type for an interval, we also need to track the type of interval
    // sentinel: the start, the end, or a single slot marker.
    const INTERVAL_SENTINEL_BITS: u32 = kVmPageListIntervalSentinelBits;
    const INTERVAL_SENTINEL_SHIFT: u32 = Self::TYPE_BITS;
    const INTERVAL_SENTINEL_MASK: u32 = (1 << Self::INTERVAL_SENTINEL_BITS) - 1;

    // Next we also need to store the type of interval being represented; reserve a couple of bits
    // for this. Currently we only support one type of interval: a range of zero pages, but
    // reserving 2 bits allows for more types in the future.
    const INTERVAL_TYPE_BITS: u32 = kVmPageListIntervalTypeBits;
    const INTERVAL_TYPE_SHIFT: u32 = Self::INTERVAL_SENTINEL_SHIFT + Self::INTERVAL_SENTINEL_BITS;
    const INTERVAL_TYPE_MASK: u32 = (1 << Self::INTERVAL_TYPE_BITS) - 1;

    const INTERVAL_BITS: u32 =
        Self::TYPE_BITS + Self::INTERVAL_SENTINEL_BITS + Self::INTERVAL_TYPE_BITS;
    const INTERVAL_MASK: u32 = (1 << Self::INTERVAL_BITS) - 1;

    const _ZERO_RANGE_ALIGN_CHECK: () =
        assert!(ZeroRange::ALIGN_BITS == VmPageOrMarker::INTERVAL_BITS);

    fn get_type(&self) -> u32 {
        self.raw & Self::TYPE_MASK
    }

    /// Creates an empty `VmPageOrMarker`.
    ///
    /// A `PAGE_TYPE` that otherwise holds a null pointer is considered to be Empty.
    pub fn empty() -> Self {
        Self { raw: Self::PAGE_TYPE }
    }

    /// Creates a `VmPageOrMarker` from a page pointer.
    pub fn from_page(p: VmPagePtr) -> Self {
        // Ensure the pmm page-to-index has enough zero bits.
        const _PMM_NODE_INDEX_CHECK: () =
            assert!(VmPageOrMarker::TYPE_BITS <= kPmmNodeIndexZeroBits);
        // A null page is incorrect for two reasons
        // 1. It's a violation of the API of this method
        // 2. A null page cannot be represented internally as this is used to represent Empty
        // (Note: `p` is non-null by `VmPagePtr` invariant)
        let raw = pmm_node().page_to_index(p);
        // Getting zero in `raw` means that `p` lives on the stack or the heap.
        // This is not supported.
        debug_assert!(raw != 0);
        // A pointer should be aligned by definition, and hence the low bits should always be zero,
        // but assert this anyway just in case TYPE_BITS is increased or someone passed an invalid
        // pointer.
        debug_assert!((raw & Self::TYPE_MASK) == 0);
        Self { raw: raw | Self::PAGE_TYPE }
    }

    /// Creates a `VmPageOrMarker` from a reference.
    pub fn from_reference(ref_val: ReferenceValue) -> Self {
        // Ensure the reference values have alignment such the type bits can be set without
        // overlapping actual ref being stored. Unlike the page type, which does not allow the 0
        // value to be stored, a ref value of 0 is valid and may be stored.
        const _REF_ALIGN_CHECK: () =
            assert!(ReferenceValue::ALIGN_BITS == VmPageOrMarker::TYPE_BITS);
        Self { raw: ref_val.value() | Self::REFERENCE_TYPE }
    }

    /// Creates a marker `VmPageOrMarker`.
    pub fn marker() -> Self {
        Self { raw: Self::ZERO_MARKER_TYPE }
    }

    /// Creates a marker `VmPageOrMarker` with a share count.
    pub fn marker_with_share_count(share_count: u32) -> Self {
        Self {
            raw: Self::ZERO_MARKER_TYPE
                | ((share_count << Self::TYPE_BITS) & Self::MARKER_SHARE_COUNT_MASK),
        }
    }

    /// Creates a parent content `VmPageOrMarker`.
    pub fn parent_content() -> Self {
        Self { raw: Self::PARENT_CONTENT_TYPE }
    }

    /// Creates a zero interval `VmPageOrMarker`.
    ///
    /// Only support creation of zero interval type for now.
    /// Private in C++ (friended with VmPageList) so callers cannot arbitrarily create sentinels.
    fn zero_interval(sentinel: SentinelType, state: ZeroRangeDirtyState) -> Self {
        let sentinel_bits = (sentinel as u32) << Self::INTERVAL_SENTINEL_SHIFT;
        let type_bits = (IntervalType::Zero as u32) << Self::INTERVAL_TYPE_SHIFT;
        let zr = ZeroRange::new_with_state(0, state);
        Self { raw: zr.value() | type_bits | sentinel_bits | Self::INTERVAL_TYPE }
    }

    /// Returns true if this is empty.
    ///
    /// A `PAGE_TYPE` that otherwise holds a null pointer is considered to be Empty.
    pub fn is_empty(&self) -> bool {
        self.raw == Self::PAGE_TYPE
    }

    /// Returns true if this is a page.
    pub fn is_page(&self) -> bool {
        !self.is_empty() && (self.get_type() == Self::PAGE_TYPE)
    }

    /// Returns true if this is a reference.
    pub fn is_reference(&self) -> bool {
        self.get_type() == Self::REFERENCE_TYPE
    }

    /// Returns true if this is a page or a reference.
    pub fn is_page_or_ref(&self) -> bool {
        self.is_page() || self.is_reference()
    }

    /// Returns true if this is a marker.
    pub fn is_marker(&self) -> bool {
        self.get_type() == Self::ZERO_MARKER_TYPE
    }

    /// Returns true if this is parent content.
    pub fn is_parent_content(&self) -> bool {
        self.get_type() == Self::PARENT_CONTENT_TYPE
    }

    /// Returns the marker share count.
    pub fn marker_share_count(&self) -> u32 {
        debug_assert!(self.is_marker());
        self.raw >> Self::TYPE_BITS
    }

    /// Sets the marker share count.
    pub fn set_marker_share_count(&mut self, share_count: u32) {
        debug_assert!(self.is_marker());
        self.raw = (self.raw & !Self::MARKER_SHARE_COUNT_MASK)
            | ((share_count << Self::TYPE_BITS) & Self::MARKER_SHARE_COUNT_MASK);
    }

    /// Increments the marker share count.
    pub fn increment_marker_share_count(&mut self) {
        debug_assert!(self.is_marker());
        self.raw += Self::MARKER_SHARE_COUNT_STEP;
    }

    /// Decrements the marker share count.
    pub fn decrement_marker_share_count(&mut self) {
        debug_assert!(self.is_marker());
        // It is invalid to decrement marker share count from zero.
        debug_assert!(self.marker_share_count() > 0);
        self.raw -= Self::MARKER_SHARE_COUNT_STEP;
    }

    /// Returns true if this is an interval.
    pub fn is_interval(&self) -> bool {
        self.get_type() == Self::INTERVAL_TYPE
    }

    /// Returns the interval sentinel type.
    fn interval_sentinel(&self) -> SentinelType {
        let bits = (self.raw >> Self::INTERVAL_SENTINEL_SHIFT) & Self::INTERVAL_SENTINEL_MASK;
        match bits {
            0 => SentinelType::Slot,
            1 => SentinelType::Start,
            2 => SentinelType::End,
            _ => panic!("invalid sentinel type"),
        }
    }

    /// Sets the interval sentinel type.
    fn set_interval_sentinel(&mut self, sentinel: SentinelType) {
        self.raw &= !(Self::INTERVAL_SENTINEL_MASK << Self::INTERVAL_SENTINEL_SHIFT);
        self.raw |= (sentinel as u32) << Self::INTERVAL_SENTINEL_SHIFT;
    }

    /// Returns the interval type.
    fn interval_type(&self) -> IntervalType {
        let bits = (self.raw >> Self::INTERVAL_TYPE_SHIFT) & Self::INTERVAL_TYPE_MASK;
        match bits {
            0 => IntervalType::Zero,
            _ => panic!("invalid interval type"),
        }
    }

    // Getters and setters for the interval type.

    /// Returns true if this is the start of an interval.
    pub fn is_interval_start(&self) -> bool {
        self.is_interval() && self.interval_sentinel() == SentinelType::Start
    }

    /// Returns true if this is the end of an interval.
    pub fn is_interval_end(&self) -> bool {
        self.is_interval() && self.interval_sentinel() == SentinelType::End
    }

    /// Returns true if this is an interval slot.
    pub fn is_interval_slot(&self) -> bool {
        self.is_interval() && self.interval_sentinel() == SentinelType::Slot
    }

    /// Returns true if this is a zero interval.
    pub fn is_interval_zero(&self) -> bool {
        self.is_interval() && self.interval_type() == IntervalType::Zero
    }

    // Getters and setter for the zero interval type.

    /// Returns true if this zero interval is clean.
    pub fn is_zero_interval_clean(&self) -> bool {
        debug_assert!(self.is_interval_zero());
        let zr_val = self.raw & !Self::INTERVAL_MASK;
        ZeroRange::new(zr_val).dirty_state() == ZeroRangeDirtyState::Clean
    }

    /// Returns true if this zero interval is dirty.
    pub fn is_zero_interval_dirty(&self) -> bool {
        debug_assert!(self.is_interval_zero());
        let zr_val = self.raw & !Self::INTERVAL_MASK;
        ZeroRange::new(zr_val).dirty_state() == ZeroRangeDirtyState::Dirty
    }

    /// Returns true if this zero interval is untracked.
    pub fn is_zero_interval_untracked(&self) -> bool {
        debug_assert!(self.is_interval_zero());
        let zr_val = self.raw & !Self::INTERVAL_MASK;
        ZeroRange::new(zr_val).dirty_state() == ZeroRangeDirtyState::Untracked
    }

    /// Returns the dirty state of this zero interval.
    pub fn zero_interval_dirty_state(&self) -> ZeroRangeDirtyState {
        debug_assert!(self.is_interval_zero());
        let zr_val = self.raw & !Self::INTERVAL_MASK;
        ZeroRange::new(zr_val).dirty_state()
    }

    /// Sets the awaiting-clean length of this zero interval.
    pub fn set_zero_interval_awaiting_clean_length(&mut self, len: u64) {
        debug_assert!(self.is_interval_zero());
        debug_assert!(self.is_interval_start() || self.is_interval_slot());
        let mut zr = ZeroRange::new(self.raw & !Self::INTERVAL_MASK);
        zr.set_awaiting_clean_length(len);
        self.raw = (self.raw & Self::INTERVAL_MASK) | zr.value();
    }

    /// Returns the awaiting-clean length of this zero interval.
    pub fn zero_interval_awaiting_clean_length(&self) -> u64 {
        debug_assert!(self.is_interval_zero());
        debug_assert!(self.is_interval_start() || self.is_interval_slot());
        let zr = ZeroRange::new(self.raw & !Self::INTERVAL_MASK);
        zr.awaiting_clean_length()
    }

    /// Change the interval sentinel type for an existing interval, while preserving the rest of the
    /// original state. Only valid to call on an existing interval type. The only permissible
    /// transitions are from Slot to Start/End and vice versa, as these are the only valid
    /// transitions when extending or clipping intervals.
    fn change_interval_sentinel(&mut self, new_sentinel: SentinelType) {
        if cfg!(debug_assertions) {
            debug_assert!(self.is_interval());
            let old_sentinel = self.interval_sentinel();
            debug_assert!(old_sentinel != new_sentinel);
            if old_sentinel == SentinelType::Start || old_sentinel == SentinelType::End {
                debug_assert!(new_sentinel == SentinelType::Slot);
            } else {
                debug_assert!(old_sentinel == SentinelType::Slot);
                debug_assert!(
                    new_sentinel == SentinelType::Start || new_sentinel == SentinelType::End
                );
            }
        }
        self.set_interval_sentinel(new_sentinel);
    }

    /// Returns the underlying page. Is only valid to call if `is_page` is true.
    ///
    /// Do not need to mask any bits out of raw, since PAGE_TYPE has 0's for the type anyway.
    pub fn page(&self) -> VmPagePtr {
        debug_assert!(self.is_page());
        // SAFETY: `self.raw` is guaranteed to be a valid page index when `self` is a page.
        unsafe { pmm_node().index_to_page(self.raw) }
    }

    /// Returns the physical address of the page. Is only valid to call if `is_page` is true.
    ///
    /// Can be more efficient than performing `page().paddr()` as it saves a memory de-reference.
    pub fn page_as_paddr(&self) -> PAddr {
        debug_assert!(self.is_page());
        // SAFETY: `self.raw` is guaranteed to be a valid page index when `self` is a page.
        unsafe { pmm_node().index_to_paddr(self.raw) }
    }

    /// Returns the underlying reference.
    pub fn reference(&self) -> ReferenceValue {
        debug_assert!(self.is_reference());
        ReferenceValue::new(self.raw & !ReferenceValue::ALIGN_MASK)
    }

    /// Resets `self` to Empty and returns the raw underlying representation.
    ///
    /// This gives up ownership of any contained page or reference without running
    /// destructor checks, allowing the caller to transfer raw contents safely.
    fn release(&mut self) -> u32 {
        let ret = self.raw;
        self.raw = Self::PAGE_TYPE;
        ret
    }

    /// If this is a page, moves the underlying `VmPagePtr` out and returns it. After this,
    /// `is_page` will be false and `is_empty` will be true.
    pub fn release_page(&mut self) -> VmPagePtr {
        debug_assert!(self.is_page());
        let raw = self.release();
        // SAFETY: `raw` is guaranteed to be a valid page index when `self` was a page.
        unsafe { pmm_node().index_to_page(raw) }
    }

    /// If this is a reference, moves the underlying reference out and returns it. After this,
    /// `is_reference` will be false and `is_empty` will be true.
    pub fn release_reference(&mut self) -> ReferenceValue {
        debug_assert!(self.is_reference());
        let raw = self.release();
        ReferenceValue::new(raw & !ReferenceValue::ALIGN_MASK)
    }

    /// Changes the content from a reference to a page and returns the original reference.
    pub fn swap_reference_for_page(&mut self, p: VmPagePtr) -> ReferenceValue {
        let ref_val = self.release_reference();
        *self = Self::from_page(p);
        ref_val
    }

    /// Changes the content from a page to a reference and returns the original page.
    pub fn swap_page_for_reference(&mut self, ref_val: ReferenceValue) -> VmPagePtr {
        let page = self.release_page();
        *self = Self::from_reference(ref_val);
        page
    }

    /// Changes the content from one reference to a different one and returns the original
    /// reference.
    pub fn swap_reference_for_reference(&mut self, ref_val: ReferenceValue) -> ReferenceValue {
        let old = self.release_reference();
        *self = Self::from_reference(ref_val);
        old
    }

    /// Swaps content with another `VmPageOrMarker`, returning the previous value of `self`.
    pub fn swap(&mut self, mut other: Self) -> Self {
        let ret = self.raw;
        self.raw = other.release();
        Self { raw: ret }
    }
}

impl Drop for VmPageOrMarker {
    fn drop(&mut self) {
        debug_assert!(
            !self.is_page_or_ref(),
            "VmPageOrMarker dropped while containing page or ref"
        );
    }
}

/// Limited reference to a `VmPageOrMarker`. This reference provides unrestricted const access to
/// the underlying `VmPageOrMarker`, but as it holds a non-const `VmPageOrMarker` pointer it has the
/// ability to modify the underlying entry. However, the interface for modification is very limited.
///
/// This allows for the majority of `VmPageList` iterations that are not intended to allow for
/// clearing entries to the Empty state to allow limited mutation (such as between different content
/// states), without being completely mutable.
pub struct VmPageOrMarkerRef<'a> {
    slot: &'a mut VmPageOrMarker,
}

impl<'a> VmPageOrMarkerRef<'a> {
    /// Creates a new `VmPageOrMarkerRef` wrapping `slot`.
    pub fn new(slot: &'a mut VmPageOrMarker) -> Self {
        Self { slot }
    }

    /// Changing the kind of content is an allowed mutation and this takes ownership of the provided
    /// page and returns ownership of the previous reference.
    pub fn swap_reference_for_page(&mut self, page: VmPagePtr) -> ReferenceValue {
        self.slot.swap_reference_for_page(page)
    }

    /// Similar to `swap_reference_for_page`, but takes ownership of the ref and returns ownership
    /// of the previous page.
    pub fn swap_page_for_reference(&mut self, ref_val: ReferenceValue) -> VmPagePtr {
        self.slot.swap_page_for_reference(ref_val)
    }

    /// Similar to `swap_reference_for_page`, but changes one reference for another.
    pub fn swap_reference_for_reference(&mut self, ref_val: ReferenceValue) -> ReferenceValue {
        self.slot.swap_reference_for_reference(ref_val)
    }

    /// Replaces the contents of this `VmPageOrMarker` with some non-empty contents, and returns
    /// what was previously present. The previous content is allowed to be empty, but the provided
    /// content must be non-empty.
    pub fn swap_content(&mut self, other: VmPageOrMarker) -> VmPageOrMarker {
        debug_assert!(!other.is_empty(), "swap_content requires non-empty content");
        self.slot.swap(other)
    }

    /// Forward dirty state updates as an allowed mutation.
    pub fn set_zero_interval_awaiting_clean_length(&mut self, len: u64) {
        self.slot.set_zero_interval_awaiting_clean_length(len);
    }

    /// Returns the share count of this marker.
    pub fn marker_share_count(&self) -> u32 {
        self.slot.marker_share_count()
    }

    /// Increments the share count of this marker.
    pub fn increment_marker_share_count(&mut self) {
        self.slot.increment_marker_share_count();
    }

    /// Decrements the share count of this marker.
    pub fn decrement_marker_share_count(&mut self) {
        self.slot.decrement_marker_share_count();
    }
}

impl<'a> Deref for VmPageOrMarkerRef<'a> {
    type Target = VmPageOrMarker;

    fn deref(&self) -> &Self::Target {
        self.slot
    }
}

/// Node in a `VmPageList` representing a contiguous 64 KiB span (16 pages) of a VMO.
#[derive(Debug)]
#[repr(C)]
pub struct VmPageListNode {
    pages: [VmPageOrMarker; kVmPageListFanOut],
}

impl VmPageListNode {
    /// Number of page slots in a node.
    pub const PAGE_FAN_OUT: usize = kVmPageListFanOut;

    /// Total size in bytes of the address range covered by a single `VmPageListNode` (64 KiB).
    pub(crate) const NODE_SPAN_BYTES: u64 = (Self::PAGE_FAN_OUT as u64) * (page::SIZE as u64);

    /// Creates a new empty `VmPageListNode`.
    pub fn new() -> Self {
        Self { pages: core::array::from_fn(|_| VmPageOrMarker::empty()) }
    }

    /// Computes the end offset for a node starting at `base_offset`.
    pub fn end_offset(base_offset: u64) -> u64 {
        debug_assert_eq!(Self::node_offset(base_offset), base_offset);
        let end = base_offset + Self::NODE_SPAN_BYTES;
        // By construction the node cannot overflow, but the compiler does not know this. By
        // explicitly telling it, some checks can be avoided as the compiler does not have to
        // consider the case where end wrapped.
        if end <= base_offset {
            // SAFETY: `base_offset` is aligned to `NODE_SPAN_BYTES` and cannot wrap past
            // `u64::MAX`.
            unsafe { core::hint::unreachable_unchecked() };
        }
        end
    }

    /// Lookup the page or marker at the specified index.
    pub fn lookup(&self, index: usize) -> &VmPageOrMarker {
        debug_assert!(index < Self::PAGE_FAN_OUT);
        &self.pages[index]
    }

    /// Lookup the mutable page or marker at the specified index.
    pub fn lookup_mut(&mut self, index: usize) -> &mut VmPageOrMarker {
        debug_assert!(index < Self::PAGE_FAN_OUT);
        &mut self.pages[index]
    }

    /// A node is empty if it contains no pages, page interval sentinels, references, or markers.
    pub fn is_empty(&self) -> bool {
        for p in &self.pages {
            if !p.is_empty() {
                return false;
            }
        }
        true
    }

    /// Returns true if there are no pages or references owned by this node. Meant to check whether
    /// the node has any resource that needs to be returned.
    pub fn has_no_page_or_ref(&self) -> bool {
        for p in &self.pages {
            if p.is_page_or_ref() {
                return false;
            }
        }
        true
    }

    /// Returns true if there are no pages, references or markers owned by this node. Meant to check
    /// whether the node has any resource that needs to be returned.
    pub fn has_no_page_ref_or_marker(&self) -> bool {
        for p in &self.pages {
            if p.is_page_or_ref() || p.is_marker() {
                return false;
            }
        }
        true
    }

    /// Returns true if there are no interval sentinels owned by this node.
    pub fn has_no_interval_sentinel(&self) -> bool {
        for p in &self.pages {
            if p.is_interval() {
                return false;
            }
        }
        true
    }

    /// For every page or marker in the node call the passed in function.
    pub fn for_every_page<F>(&self, base: u64, func: F) -> Status
    where
        F: FnMut(&VmPageOrMarker, u64) -> Status,
    {
        self.for_every_page_in_range(base, base, Self::end_offset(base), func)
    }

    /// For every page or marker in the node call the passed in function.
    pub fn for_every_page_ref<F>(&mut self, base: u64, func: F) -> Status
    where
        F: FnMut(VmPageOrMarkerRef<'_>, u64) -> Status,
    {
        self.for_every_page_in_range_ref(base, base, Self::end_offset(base), func)
    }

    /// For every page or marker in the node in the range call the passed in function. The range
    /// is assumed to be within the node's object range.
    pub fn for_every_page_in_range<F>(
        &self,
        base: u64,
        start_offset: u64,
        end_offset: u64,
        mut func: F,
    ) -> Status
    where
        F: FnMut(&VmPageOrMarker, u64) -> Status,
    {
        debug_assert!(end_offset >= start_offset);
        debug_assert!(start_offset >= base);
        debug_assert!(end_offset <= Self::end_offset(base));
        let start = ((start_offset - base) / (page::SIZE as u64)) as usize;
        let end = ((end_offset - base) / (page::SIZE as u64)) as usize;
        #[allow(clippy::needless_range_loop)]
        for i in start..end {
            if !self.pages[i].is_empty() {
                let status = func(&self.pages[i], base + (i as u64) * (page::SIZE as u64));
                if status != Status::NEXT {
                    return status;
                }
            }
        }
        Status::NEXT
    }

    /// For every page or marker in the node in the range call the passed in function with a
    /// [`VmPageOrMarkerRef`]. The range is assumed to be within the node's object range.
    pub fn for_every_page_in_range_ref<F>(
        &mut self,
        base: u64,
        start_offset: u64,
        end_offset: u64,
        mut func: F,
    ) -> Status
    where
        F: FnMut(VmPageOrMarkerRef<'_>, u64) -> Status,
    {
        self.for_every_page_in_range_mut(base, start_offset, end_offset, |slot, offset| {
            func(VmPageOrMarkerRef::new(slot), offset)
        })
    }

    /// For every page or marker in the node in the range call the passed in function with
    /// mutable access. The range is assumed to be within the node's object range.
    pub fn for_every_page_in_range_mut<F>(
        &mut self,
        base: u64,
        start_offset: u64,
        end_offset: u64,
        mut func: F,
    ) -> Status
    where
        F: FnMut(&mut VmPageOrMarker, u64) -> Status,
    {
        debug_assert!(end_offset >= start_offset);
        debug_assert!(start_offset >= base);
        debug_assert!(end_offset <= Self::end_offset(base));
        let start = ((start_offset - base) / (page::SIZE as u64)) as usize;
        let end = ((end_offset - base) / (page::SIZE as u64)) as usize;
        #[allow(clippy::needless_range_loop)]
        for i in start..end {
            if !self.pages[i].is_empty() {
                let status = func(&mut self.pages[i], base + (i as u64) * (page::SIZE as u64));
                if status != Status::NEXT {
                    return status;
                }
            }
        }
        Status::NEXT
    }

    /// Checks if the given offset is part of an interval involving this node. This method cannot
    /// find the full interval, since that may require looking at an additional node, but can
    /// determine if in an interval or not. Returns any interval sentinel found, otherwise `None`.
    pub fn is_offset_in_interval(&self, obj_offset: u64, off: u64) -> Option<&VmPageOrMarker> {
        debug_assert!(off >= obj_offset);
        debug_assert!(off < Self::end_offset(obj_offset));
        let index = ((off - obj_offset) / (page::SIZE as u64)) as usize;
        // If the target slot is any kind of interval (start, end, individual slot), then we are in
        // an interval.
        if !self.pages[index].is_empty() {
            return if self.pages[index].is_interval() { Some(&self.pages[index]) } else { None };
        }
        // Check if there is an interval end to the right, which would cause this to be in an
        // interval. Finding anything else indicates we cannot be in an interval.
        #[allow(clippy::needless_range_loop)]
        for i in (index + 1)..Self::PAGE_FAN_OUT {
            if !self.pages[i].is_empty() {
                return if self.pages[i].is_interval_end() { Some(&self.pages[i]) } else { None };
            }
        }
        // Nothing to our right, so check for an interval start to our left.
        for i in (0..index).rev() {
            if !self.pages[i].is_empty() {
                return if self.pages[i].is_interval_start() { Some(&self.pages[i]) } else { None };
            }
        }
        panic!("Unexpected empty node");
    }

    /// Check if this node begins in an interval, that is if an interval start was in a preceding
    /// node and this nodes contains the end. If the first non-empty slot is an interval end it is
    /// returned, otherwise we cannot have started in an interval and `None` is returned.
    pub fn node_starts_in_interval(&self) -> Option<&VmPageOrMarker> {
        for p in &self.pages {
            if !p.is_empty() {
                return if p.is_interval_end() { Some(p) } else { None };
            }
        }
        panic!("Unexpected empty node");
    }

    #[allow(clippy::too_many_arguments)]
    pub fn merge_range_onto<F>(
        &mut self,
        base: u64,
        other_base: u64,
        mut migrate_fn: F,
        other: &mut VmPageListNode,
        start_offset: u64,
        end_offset: u64,
        other_start_offset: u64,
    ) where
        F: FnMut(&mut VmPageOrMarker, &mut VmPageOrMarker, u64),
    {
        debug_assert!(other_start_offset >= other_base);
        debug_assert!(
            other_start_offset + (end_offset - start_offset) <= Self::end_offset(other_base)
        );
        let _ = self.for_every_page_in_range_mut(base, start_offset, end_offset, |slot, offset| {
            let other_offset = offset - start_offset + other_start_offset;
            debug_assert_eq!(Self::node_offset(other_offset), other_base);
            let other_index = Self::node_index(other_offset);
            migrate_fn(slot, &mut other.pages[other_index], other_offset);
            Status::NEXT
        });
    }

    /// Converts the supplied offset into a VmPageListNode base offset.
    pub const fn node_offset(offset: u64) -> u64 {
        offset & !(Self::NODE_SPAN_BYTES - 1)
    }

    /// Converts the supplied offset into a VmPageListNode index.
    pub const fn node_index(offset: u64) -> usize {
        ((offset >> (page::SHIFT as u32)) % (Self::PAGE_FAN_OUT as u64)) as usize
    }
}

impl Default for VmPageListNode {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for VmPageListNode {
    fn drop(&mut self) {
        debug_assert!(
            self.has_no_page_or_ref(),
            "VmPageListNode dropped while containing page or ref"
        );
    }
}

/// A sparse list of pages organized into 64 KiB (16-page) `VmPageListNode` chunks.
#[repr(C)]
pub struct VmPageList {
    list: Opaque<bindings::VmPageListBtree>,
}

zr::static_assert!(core::mem::size_of::<VmPageListNode>() == 64);
zr::static_assert!(core::mem::align_of::<VmPageListNode>() == 4);
zr::static_assert!(
    core::mem::size_of::<VmPageList>() == core::mem::size_of::<bindings::VmPageListBtree>()
);

struct NodeIter<'a> {
    cursor: Opaque<bindings::VmPageListBtreeConstCursor>,
    _phantom: core::marker::PhantomData<&'a VmPageList>,
}

impl<'a> Iterator for NodeIter<'a> {
    type Item = &'a VmPageListNode;

    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY: `self.cursor.get()` is a valid initialized cursor pointer.
        let entry =
            unsafe { bindings::cpp_vm_page_list_btree_const_cursor_next(self.cursor.get()) };
        if entry.node.is_null() {
            None
        } else {
            // SAFETY: `entry.node` is verified non-null and valid for lifetime `'a`.
            Some(unsafe { entry.node.cast::<VmPageListNode>().as_ref_unchecked() })
        }
    }
}

struct NodeIterMut<'a> {
    cursor: Opaque<bindings::VmPageListBtreeCursor>,
    _phantom: core::marker::PhantomData<&'a mut VmPageList>,
}

impl<'a> Iterator for NodeIterMut<'a> {
    type Item = &'a mut VmPageListNode;

    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY: `self.cursor.get()` is a valid initialized cursor pointer.
        let entry = unsafe { bindings::cpp_vm_page_list_btree_cursor_next(self.cursor.get()) };
        if entry.node.is_null() {
            None
        } else {
            // SAFETY: `entry.node` is verified non-null and valid for lifetime `'a`.
            Some(unsafe { entry.node.cast::<VmPageListNode>().as_mut_unchecked() })
        }
    }
}

/// Helper object for performing repeated `lookup_or_allocate` operations that are likely to be
/// close to each other. While using this object other (modifying) methods on this `VmPageList` must
/// not be performed, if they are the `reset` method needs to be used before continuing.
pub struct BatchInserter<'a> {
    list: &'a mut VmPageList,
    node: Opaque<bindings::VmPageListBtreeCursor>,
}

const _: () = {
    assert!(core::mem::size_of::<BatchInserter<'_>>() == 24);
    assert!(core::mem::align_of::<BatchInserter<'_>>() == 8);
};

impl<'a> BatchInserter<'a> {
    /// Construct a `BatchInserter` for the specified `VmPageList`. The list must be kept alive for
    /// the duration of this object.
    pub fn new(list: &'a mut VmPageList) -> Self {
        let cursor = Opaque::uninit();
        // SAFETY: `cursor.get()` is a valid pointer to be initialized to default invalid iterator.
        unsafe { bindings::cpp_vm_page_list_btree_cursor_default_init(cursor.get()) };
        Self { list, node: cursor }
    }

    /// Similar to `VmPageList::lookup_or_allocate` but is implicitly `NoIntervals`. If repeated
    /// offsets are 'near' each other (in the same node, or in following nodes) then this will be
    /// more efficient than `VmPageList::lookup_or_allocate`. However, regardless of the `offset`
    /// pattern this will always return correct results.
    pub fn lookup_or_allocate(&mut self, offset: u64) -> Option<&mut VmPageOrMarker> {
        let target_offset = VmPageListNode::node_offset(offset);
        let index = VmPageListNode::node_index(offset);

        // Assume we're going to need to search for a new iterator.
        let mut search = true;

        // First check if the currently saved iterator is valid and for the correct node.
        // SAFETY: `self.node.get()` is a valid pointer.
        let mut entry = unsafe { bindings::cpp_vm_page_list_btree_cursor_get(self.node.get()) };
        if !entry.node.is_null() {
            if entry.offset == target_offset {
                // SAFETY: `entry.node` is verified non-null and valid.
                let node = unsafe { entry.node.cast::<VmPageListNode>().as_mut_unchecked() };
                return Some(node.lookup_mut(index));
            }
            // Under the assumption of contiguous insertion, check if incrementing the iterator
            // helps.
            if entry.offset < target_offset {
                // Advance the cursor to the next node.
                // SAFETY: `self.node.get()` is a valid pointer.
                let _ = unsafe { bindings::cpp_vm_page_list_btree_cursor_next(self.node.get()) };
                // Peek at the new node.
                // SAFETY: `self.node.get()` is a valid pointer.
                entry = unsafe { bindings::cpp_vm_page_list_btree_cursor_get(self.node.get()) };
                if entry.node.is_null() {
                    // If we hit the end then we know a new node is needed, so skip the search and
                    // go straight to new node creation.
                    search = false;
                } else if entry.offset == target_offset {
                    // SAFETY: `entry.node` is verified non-null and valid.
                    let node = unsafe { entry.node.cast::<VmPageListNode>().as_mut_unchecked() };
                    return Some(node.lookup_mut(index));
                }
            }
        }

        // Unless we know that the node we want isn't in the tree we must do a search for it to
        // avoid duplicate insertion.
        if search {
            // Even if this is not our target node, we stash the result to use to optimize the
            // insertion later.
            // SAFETY: `self.list.list.get()` and `self.node.get()` are valid pointers.
            entry = unsafe {
                bindings::cpp_vm_page_list_btree_lower_bound(
                    self.list.list.get(),
                    target_offset,
                    self.node.get(),
                )
            };
            if !entry.node.is_null() && entry.offset == target_offset {
                // SAFETY: `entry.node` is verified non-null and valid.
                let node = unsafe { entry.node.cast::<VmPageListNode>().as_mut_unchecked() };
                return Some(node.lookup_mut(index));
            }
        }

        // SAFETY: `self.list.list.get()` and `self.node.get()` are valid pointers.
        let node_ptr = unsafe {
            bindings::cpp_vm_page_list_btree_insert(
                self.list.list.get(),
                target_offset,
                self.node.get(),
            )
        };
        if node_ptr.is_null() {
            return None;
        }
        // SAFETY: `node_ptr` was allocated by `VmPageListNode::Create()` and inserted into BTree.
        let node = unsafe { node_ptr.cast::<VmPageListNode>().as_mut_unchecked() };
        Some(node.lookup_mut(index))
    }

    /// Reset the batch inserter. This makes it safe to use again if other `VmPageList` operations
    /// had been performed.
    pub fn reset(&mut self) {
        // SAFETY: `self.node.get()` is a valid pointer to be reset to default invalid iterator.
        unsafe { bindings::cpp_vm_page_list_btree_cursor_default_init(self.node.get()) };
    }
}

/// The interval handling flag to be used by `lookup_or_allocate`. See comments near
/// `lookup_or_allocate`.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IntervalHandling {
    NoIntervals,
    CheckForInterval,
    SplitInterval,
}

/// Behavior for checking and cleaning up empty nodes during range traversals.
trait NodeCheck {
    const CLEANUP_EMPTY: bool;
}

/// Traverse nodes without checking for or removing empty nodes.
enum SkipNodeCheck {}
impl NodeCheck for SkipNodeCheck {
    const CLEANUP_EMPTY: bool = false;
}

/// Check if nodes are empty after running the callback and erase them if so.
enum CleanupEmptyNodeCheck {}
impl NodeCheck for CleanupEmptyNodeCheck {
    const CLEANUP_EMPTY: bool = true;
}

const fn round_down(val: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    val & !(align - 1)
}

impl VmPageList {
    /// Allow the implementation to use a one-past-the-end for VmPageListNode offsets.
    pub const MAX_SIZE: u64 = round_down(u64::MAX, VmPageListNode::NODE_SPAN_BYTES);

    /// Constructor. Creates a new empty `VmPageList`.
    pub fn new() -> Self {
        let list = Self { list: Opaque::uninit() };
        // SAFETY: `list.list.get()` points to valid storage allocated for VmPageListBtree.
        unsafe {
            bindings::cpp_vm_page_list_btree_init(list.list.get().cast());
        }
        ltracef!("{:p}\n", core::ptr::from_ref(&list));
        list
    }

    fn nodes(&self) -> NodeIter<'_> {
        let cursor = Opaque::uninit();
        // SAFETY: `self.list.get()` is a valid initialized BTree, `cursor.get()` is uninitialized
        // storage.
        unsafe {
            bindings::cpp_vm_page_list_btree_const_cursor_init(cursor.get(), self.list.get());
        }
        NodeIter { cursor, _phantom: core::marker::PhantomData }
    }

    fn nodes_mut(&mut self) -> NodeIterMut<'_> {
        let cursor = Opaque::uninit();
        // SAFETY: `self.list.get()` is a valid initialized BTree, `cursor.get()` is uninitialized
        // storage.
        unsafe {
            bindings::cpp_vm_page_list_btree_cursor_init(cursor.get(), self.list.get());
        }
        NodeIterMut { cursor, _phantom: core::marker::PhantomData }
    }

    /// Returns true if there are no pages, references, markers, or intervals in the page list.
    pub fn is_empty(&self) -> bool {
        // SAFETY: `self.list.get()` is a valid initialized VmPageListBtree pointer.
        unsafe { bindings::cpp_vm_page_list_btree_is_empty(self.list.get()) }
    }

    /// Returns true if the page list does not own any pages or references. Meant to check whether
    /// the page list has any resource that needs to be returned.
    pub fn has_no_page_or_ref(&self) -> bool {
        self.nodes().all(|node| node.has_no_page_or_ref())
    }

    /// Similar to `has_no_page_or_ref` but returns false if there is a marker.
    pub fn has_no_page_ref_or_marker(&self) -> bool {
        self.nodes().all(|node| node.has_no_page_ref_or_marker())
    }

    /// Clears the tree of any remaining slots, leaving it in the initially allocated state. It is
    /// an error, and will trigger a panic, for any of the slots to hold pages or references, as
    /// clearing them would otherwise result in a memory leak.
    pub fn clear(&mut self) {
        // SAFETY: `self.list.get()` is a valid initialized VmPageListBtree pointer.
        unsafe {
            bindings::cpp_vm_page_list_btree_clear(self.list.get());
        }
    }

    /// Attempts to return a reference to the `VmPageOrMarker` at the specified offset. The returned
    /// pointer is valid until the `VmPageList` is destroyed or any of the Remove*/Take/Merge etc
    /// functions are called.
    ///
    /// Lookup may return None if there is no slot allocated for the given offset. If Some is
    /// returned it may still be the case that `is_empty` on the returned `VmPageOrMarker` is true.
    pub fn lookup(&self, offset: u64) -> Option<&VmPageOrMarker> {
        let node_offset = VmPageListNode::node_offset(offset);
        // SAFETY: `self.list.get()` is valid and `cpp_vm_page_list_btree_find_const` returns a
        // valid node or null.
        let node_ptr =
            unsafe { bindings::cpp_vm_page_list_btree_find_const(self.list.get(), node_offset) };
        if node_ptr.is_null() {
            return None;
        }
        // SAFETY: `node_ptr` is verified non-null and points to a valid `VmPageListNode`.
        let node_ref: &VmPageListNode =
            unsafe { node_ptr.cast::<VmPageListNode>().as_ref_unchecked() };
        Some(node_ref.lookup(VmPageListNode::node_index(offset)))
    }

    /// Similar to `lookup` but returns a `VmPageOrMarkerRef` that allows for limited mutation of
    /// the slot. General mutation requires calling `lookup_or_allocate`.
    pub fn lookup_mut(&mut self, offset: u64) -> Option<VmPageOrMarkerRef<'_>> {
        let slot_ptr = self.lookup_slot(offset);
        if slot_ptr.is_null() {
            None
        } else {
            // SAFETY: `slot_ptr` is verified non-null and points to a valid `VmPageOrMarker`.
            Some(VmPageOrMarkerRef::new(unsafe { &mut *slot_ptr }))
        }
    }

    /// Similar to `lookup` but only returns None if a slot cannot be allocated either due to out
    /// of memory, due to offset being invalid, or `interval_handling` not allowing for a slot to
    /// be safely returned.
    ///
    /// The returned slot, if not None, may generally be freely manipulated with the exception
    /// that if it started `!is_empty()`, then it is an error to set it to `is_empty()`. In this
    /// case the `remove_page` method must be used.
    ///
    /// If the returned slot started `is_empty()`, and is not made `!is_empty()`, then the slot must
    /// be returned with `return_empty_slot`, to ensure no empty nodes are retained.
    ///
    /// The bool in the return tuple returns whether the offset falls inside a sparse interval.
    /// And whether a valid `VmPageOrMarker` is returned in the return tuple depends on the
    /// specified `interval_handling`.
    ///  - NoIntervals: The page list does not contain any intervals, so there is no special
    ///    handling to check for or split intervals. In other words, each slot in the page list can
    ///    be manipulated independently.
    ///  - CheckForInterval: The page list can contain intervals, and the bool in the returned
    ///    tuple indicates whether the offset fell inside an interval. Note that this only checks
    ///    for intervals but does not allow manipulating them, so a valid `VmPageOrMarker` will be
    ///    returned only if the offset can safely be manipulated independently.
    ///  - SplitInterval: The page list can contain intervals and we are allowed to split
    ///    intervals to return the required slot. The returned `VmPageOrMarker` can be manipulated
    ///    freely. (See comments near `lookup_or_allocate_check_for_interval` for an explanation of
    ///    how splitting works.)
    pub fn lookup_or_allocate(
        &mut self,
        offset: u64,
        interval_handling: IntervalHandling,
    ) -> (Option<&mut VmPageOrMarker>, bool) {
        match interval_handling {
            IntervalHandling::NoIntervals => (self.lookup_or_allocate_internal(offset), false),
            IntervalHandling::CheckForInterval => {
                self.lookup_or_allocate_check_for_interval(offset, false)
            }
            IntervalHandling::SplitInterval => {
                self.lookup_or_allocate_check_for_interval(offset, true)
            }
        }
    }

    /// Internal helper for `lookup_or_allocate`.
    fn lookup_or_allocate_internal(&mut self, offset: u64) -> Option<&mut VmPageOrMarker> {
        let node_offset = VmPageListNode::node_offset(offset);
        let index = VmPageListNode::node_index(offset);

        ltracef_level!(
            2,
            "{:p} offset {:#x} node_offset {:#x} index {}\n",
            core::ptr::from_ref(self),
            offset,
            node_offset,
            index
        );

        if node_offset >= Self::MAX_SIZE {
            return None;
        }

        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // lookup the tree node that holds this page. Use lower_bound instead of find to optimize
        // later insertion in case of failed lookup.
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let entry = unsafe {
            bindings::cpp_vm_page_list_btree_lower_bound(self.list.get(), node_offset, cursor.get())
        };
        if !entry.node.is_null() && entry.offset == node_offset {
            // SAFETY: `entry.node` is verified non-null and points to a valid `VmPageListNode`.
            let node = unsafe { entry.node.cast::<VmPageListNode>().as_mut_unchecked() };
            return Some(node.lookup_mut(index));
        }

        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let node_ptr = unsafe {
            bindings::cpp_vm_page_list_btree_insert(self.list.get(), node_offset, cursor.get())
        };
        if node_ptr.is_null() {
            return None;
        }
        // SAFETY: `node_ptr` was allocated by `VmPageListNode::Create()` and inserted into BTree.
        let node_ref: &mut VmPageListNode =
            unsafe { node_ptr.cast::<VmPageListNode>().as_mut_unchecked() };
        Some(node_ref.lookup_mut(index))
    }

    /// Similar to `lookup_or_allocate_internal` but also checks if `offset` falls in a sparse page
    /// interval, returning true via the bool in the tuple if it does, along with the slot. Also
    /// splits the interval around `offset` if `split_interval` is set to true. This allows the
    /// caller to freely manipulate the slot at `offset` similar to `lookup_or_allocate`. If
    /// `offset` is found in an interval, but `split_interval` was false, no `VmPageOrMarker` is
    /// returned, as it is not safe to manipulate any slot in an interval without also splitting the
    /// interval around it.
    ///
    /// In other words, the return values fall into three categories:
    ///  1. `(Some(page), false)`: `offset` does not lie in an interval. `page` is the required
    ///     slot.
    ///  2. `(Some(page), true)`: `offset` lies in an interval and `split_interval` was true. `page`
    ///     is the required slot. The interval has been correctly split around the slot, so `page`
    ///     can be treated similar to any non-interval type.
    ///  3. `(None, true)`: `offset` lies in an interval but `split_interval` was false. No slot is
    ///     returned.
    ///
    /// Splitting the interval would look as follows. If the interval previously was:
    ///  `[start, end)` where `start < offset < end`.
    /// After the split we would have three intervals:
    ///  `[start, offset) [offset, offset + page::SIZE) [offset + page::SIZE, end)`
    /// The middle interval containing `offset` spans only a single page, i.e. `offset` is an
    /// `SentinelType::Slot`, which can now be manipulated independently.
    fn lookup_or_allocate_check_for_interval(
        &mut self,
        offset: u64,
        split_interval: bool,
    ) -> (Option<&mut VmPageOrMarker>, bool) {
        // Find the node that would contain this offset.
        let node_offset = VmPageListNode::node_offset(offset);
        let node_index = VmPageListNode::node_index(offset);
        if node_offset >= Self::MAX_SIZE {
            return (None, false);
        }

        // If the node containing offset is populated, the lower bound will return that node. If not
        // populated, it will return the next populated node.
        //
        // The overall intent with this function is to keep the number of tree lookups similar to
        // lookup_or_allocate for both empty and non-empty slots. So we hold on to the looked up
        // node and walk left or right in the tree only if required. The same principle is followed
        // for traversal within a node as well; the ordering of operations is chosen such that we
        // can exit the traversal as soon as possible or avoid it entirely.
        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let entry = unsafe {
            bindings::cpp_vm_page_list_btree_lower_bound(self.list.get(), node_offset, cursor.get())
        };

        // The slot that will eventually hold offset.
        let mut slot: *mut VmPageOrMarker = core::ptr::null_mut();

        // If offset falls in an interval, this will hold the dirty state of the interval that
        // offset is found in. It will be used to mint new sentinel values if we were also asked to
        // split the interval.
        let mut found_interval_dirty_state = ZeroRangeDirtyState::Untracked;
        let mut is_in_interval = false;

        // For the offset to lie in an interval, it is going to have an interval end so we should
        // have found some valid node if the offset falls in an interval. If we could not find a
        // valid node, we know that offset cannot lie in an interval, so skip the check.
        if !entry.node.is_null() {
            let node_ptr = entry.node.cast::<VmPageListNode>();
            if entry.offset == node_offset {
                // We found the node containing offset. Get the slot.
                let pages_ptr =
                    unsafe { core::ptr::addr_of_mut!((*node_ptr).pages).cast::<VmPageOrMarker>() };
                slot = unsafe { pages_ptr.add(node_index) };
                // Short circuit the is_offset_in_interval_helper call below if the slot itself is
                // an interval sentinel. This is purely an optimization, and it would be okay to
                // call is_offset_in_interval_helper for this case too.
                // SAFETY: `slot` is non-null and valid.
                if unsafe { (*slot).is_interval() } {
                    is_in_interval = true;
                    found_interval_dirty_state = unsafe { (*slot).zero_interval_dirty_state() };
                }
            }

            if !is_in_interval {
                // SAFETY: `entry.node` is non-null and points to a valid `VmPageListNode`.
                let node = unsafe { entry.node.cast::<VmPageListNode>().as_ref_unchecked() };
                if let Some(interval) =
                    Self::is_offset_in_interval_helper(offset, entry.offset, node)
                {
                    is_in_interval = true;
                    debug_assert!(interval.is_interval());
                    found_interval_dirty_state = interval.zero_interval_dirty_state();
                }
            }

            // If we are in an interval but cannot split it, we cannot return a slot. The caller
            // should not be able to manipulate the slot freely without correctly handling the
            // interval(s) around it.
            if is_in_interval && !split_interval {
                return (None, true);
            }
        }

        // We won't have a valid slot if the node we looked up did not contain the required offset.
        if slot.is_null() {
            // Allocate the node that would contain offset and then get the slot.
            // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
            let node_ptr = unsafe {
                bindings::cpp_vm_page_list_btree_insert(self.list.get(), node_offset, cursor.get())
            };
            if node_ptr.is_null() {
                return (None, is_in_interval);
            }
            let node_ptr = node_ptr.cast::<VmPageListNode>();
            let pages_ptr =
                unsafe { core::ptr::addr_of_mut!((*node_ptr).pages).cast::<VmPageOrMarker>() };
            slot = unsafe { pages_ptr.add(node_index) };
        }

        // If offset does not lie in an interval, or if the slot is already a single page interval,
        // there is nothing more to be done. Return the slot.
        // SAFETY: `slot` is non-null and valid.
        if !is_in_interval || unsafe { (*slot).is_interval_slot() } {
            // We currently only support zero intervals.
            debug_assert!(!is_in_interval || unsafe { (*slot).is_interval_zero() });
            // SAFETY: `slot` is non-null and valid for the lifetime of `&mut self`.
            return (Some(unsafe { &mut *slot }), is_in_interval);
        }

        // If we reached here, we know that we are in an interval and we need to split it in order
        // to return the required slot.
        debug_assert!(is_in_interval && split_interval);

        // Depending on whether the slot is empty or not, we might need to insert a new interval
        // start, a new interval end, or both. Figure out which slots are needed first.
        //  - If the slot is empty, we need to insert an end to the left, a start to the right and a
        //    single page interval slot at offset. So we need both a new start and a new end.
        //  - If the slot is populated, since we know that offset falls in an interval, it could
        //    only be an interval start or an interval end. We will either need a new start or a new
        //    end but not both.
        let mut need_new_end = true;
        let mut need_new_start = true;
        // SAFETY: `slot` is non-null and valid.
        if unsafe { (*slot).is_interval_start() } {
            // We can move the interval start to the right, and replace the old start with a slot.
            // Don't need a new end.
            need_new_end = false;
        } else if unsafe { (*slot).is_interval_end() } {
            // We can move the interval end to the left, and replace the old end with a slot. Don't
            // need a new start.
            need_new_start = false;
        }

        // Now find the previous and next slots as needed for the new end and new start
        // respectively.
        // Note that the node allocations below are mutually exclusive. If we allocate the previous
        // node, we won't need to allocate the next node and vice versa, since the allocation logic
        // depends on the value of node_index. So if the required node allocation fails, all the
        // cleanup that's required is returning the slot at offset if it is empty, as we might have
        // allocated a new node previously to hold offset.
        let mut new_end: *mut VmPageOrMarker = core::ptr::null_mut();
        if need_new_end {
            if node_index > 0 {
                // The previous slot is in the same node.
                // SAFETY: `slot` is non-null and node_index > 0, so slot.sub(1) points to a valid
                // slot in the same node.
                new_end = unsafe { slot.sub(1) };
            } else {
                // The previous slot is in the node to the left. We might need to allocate a new
                // node to the left if it does not exist. Try to walk left and see if we find the
                // previous node we're looking for.
                let prev_node_offset = node_offset - VmPageListNode::NODE_SPAN_BYTES;
                // SAFETY: `cursor.get()` is a valid pointer pointing to current node.
                let prev_entry =
                    unsafe { bindings::cpp_vm_page_list_btree_cursor_prev(cursor.get()) };
                // We are here because slot was either an empty slot inside an interval or it was an
                // interval end. Additionally, the slot was the left-most slot in its node, which
                // means we are guaranteed to find a node to the left which holds the start of the
                // interval.
                debug_assert!(!prev_entry.node.is_null());
                if prev_entry.offset == prev_node_offset {
                    let prev_node = prev_entry.node.cast::<VmPageListNode>();
                    // SAFETY: `prev_node` is non-null.
                    let pages_ptr = unsafe {
                        core::ptr::addr_of_mut!((*prev_node).pages).cast::<VmPageOrMarker>()
                    };
                    new_end = unsafe { pages_ptr.add(VmPageListNode::PAGE_FAN_OUT - 1) };
                } else {
                    debug_assert!(prev_entry.offset < prev_node_offset);
                    // SAFETY: `self.list.get()` is a valid pointer. Passing null cursor inserts
                    // without a hint.
                    let node_ptr = unsafe {
                        bindings::cpp_vm_page_list_btree_insert(
                            self.list.get(),
                            prev_node_offset,
                            core::ptr::null_mut(),
                        )
                    };
                    if node_ptr.is_null() {
                        // SAFETY: `slot` is non-null.
                        if unsafe { (*slot).is_empty() } {
                            self.return_empty_slot(offset);
                        }
                        return (None, true);
                    }
                    let node_ptr = node_ptr.cast::<VmPageListNode>();
                    // SAFETY: `node_ptr` is non-null.
                    let pages_ptr = unsafe {
                        core::ptr::addr_of_mut!((*node_ptr).pages).cast::<VmPageOrMarker>()
                    };
                    new_end = unsafe { pages_ptr.add(VmPageListNode::PAGE_FAN_OUT - 1) };
                }
            }
            debug_assert!(!new_end.is_null());
        }

        let mut new_start: *mut VmPageOrMarker = core::ptr::null_mut();
        if need_new_start {
            if node_index < VmPageListNode::PAGE_FAN_OUT - 1 {
                // The next slot is in the same node.
                // SAFETY: `slot` is non-null and node_index < PAGE_FAN_OUT - 1, so slot.add(1)
                // points to a valid slot in the same node.
                new_start = unsafe { slot.add(1) };
            } else {
                // The next slot is in the node to the right. We might need to allocate a new node
                // to the right if it does not exist. Try to walk right and see if we find the next
                // node we're looking for.
                let next_node_offset = node_offset + VmPageListNode::NODE_SPAN_BYTES;
                // SAFETY: `cursor.get()` is a valid pointer pointing to current node.
                let _ = unsafe { bindings::cpp_vm_page_list_btree_cursor_next(cursor.get()) };
                let next_entry =
                    unsafe { bindings::cpp_vm_page_list_btree_cursor_get(cursor.get()) };
                // We are here because slot was either empty or it was an interval start.
                // Additionally, the slot was the right-most slot in its node, which means we are
                // guaranteed to find a node to the right which holds the end of the interval.
                debug_assert!(!next_entry.node.is_null());
                if next_entry.offset == next_node_offset {
                    let next_node = next_entry.node.cast::<VmPageListNode>();
                    // SAFETY: `next_node` is non-null.
                    new_start = unsafe {
                        core::ptr::addr_of_mut!((*next_node).pages).cast::<VmPageOrMarker>()
                    };
                } else {
                    debug_assert!(next_entry.offset > next_node_offset);
                    // SAFETY: `self.list.get()` is a valid pointer. Passing null cursor inserts
                    // without a hint.
                    let node_ptr = unsafe {
                        bindings::cpp_vm_page_list_btree_insert(
                            self.list.get(),
                            next_node_offset,
                            core::ptr::null_mut(),
                        )
                    };
                    if node_ptr.is_null() {
                        // SAFETY: `slot` is non-null.
                        if unsafe { (*slot).is_empty() } {
                            self.return_empty_slot(offset);
                        }
                        return (None, true);
                    }
                    let node_ptr = node_ptr.cast::<VmPageListNode>();
                    // SAFETY: `node_ptr` is non-null.
                    new_start = unsafe {
                        core::ptr::addr_of_mut!((*node_ptr).pages).cast::<VmPageOrMarker>()
                    };
                }
            }
            debug_assert!(!new_start.is_null());
        }

        // Now that we've looked up the relevant slots after performing any required allocations,
        // make the actual change. Install new end and start sentinels on the left and right of
        // offset respectively.
        if !new_start.is_null() {
            // SAFETY: `new_start` is non-null and valid.
            let new_start_ref = unsafe { &mut *new_start };
            if new_start_ref.is_interval_end() {
                // If an interval was ending at the next slot, change it into a Slot sentinel.
                new_start_ref.change_interval_sentinel(SentinelType::Slot);
            } else {
                debug_assert!(new_start_ref.is_empty());
                *new_start_ref =
                    VmPageOrMarker::zero_interval(SentinelType::Start, found_interval_dirty_state);
            }
        }
        if !new_end.is_null() {
            // SAFETY: `new_end` is non-null and valid.
            let end_slot_ref = unsafe { &mut *new_end };
            if end_slot_ref.is_interval_start() {
                // If an interval was starting at the previous slot, change it into a Slot sentinel.
                end_slot_ref.change_interval_sentinel(SentinelType::Slot);
            } else {
                debug_assert!(end_slot_ref.is_empty());
                *end_slot_ref =
                    VmPageOrMarker::zero_interval(SentinelType::End, found_interval_dirty_state);
            }
        }

        // Finally, install a slot sentinel at offset.
        // SAFETY: `slot` is non-null and valid for the lifetime of `&mut self`.
        let slot_ref = unsafe { &mut *slot };
        if slot_ref.is_empty() {
            *slot_ref =
                VmPageOrMarker::zero_interval(SentinelType::Slot, found_interval_dirty_state);
        } else {
            debug_assert!(slot_ref.is_interval_start() || slot_ref.is_interval_end());
            // If we're overwriting the start or end sentinel, carry over any relevant state
            // information to the rest of the interval that remains (if required).
            //
            // For zero intervals, this means preserving any non-zero awaiting_clean_length in the
            // start sentinel. We only need to do this if the zero interval is being split at the
            // start. This is an optimization to avoid having to potentially walk to another node
            // to find the relevant start to update. So the awaiting_clean_length can be larger
            // than the length of the resultant interval; the caller will take that into account
            // and carry over larger awaiting_clean_lengths across multiple intervals if they
            // exist. (See related comment in VmCowPages::WritebackEndLocked.)
            if slot_ref.is_interval_start() {
                let awaiting_clean_len = slot_ref.zero_interval_awaiting_clean_length();
                if awaiting_clean_len > page::SIZE as u64 {
                    debug_assert!(!new_start.is_null());
                    // SAFETY: `new_start` is non-null when `slot_ref.is_interval_start()`.
                    let new_start_ref = unsafe { &mut *new_start };
                    new_start_ref.set_zero_interval_awaiting_clean_length(
                        awaiting_clean_len - page::SIZE as u64,
                    );
                    slot_ref.set_zero_interval_awaiting_clean_length(page::SIZE as u64);
                }
            }
            slot_ref.change_interval_sentinel(SentinelType::Slot);
        }

        (Some(slot_ref), true)
    }

    /// Returns a slot that was empty after `lookup_or_allocate`, and that the caller did not end up
    /// filling.
    /// This ensures that if `lookup_or_allocate` allocated a new underlying list node, then that
    /// list node needs to be free'd otherwise it might not get cleaned up for the lifetime of the
    /// page list.
    ///
    /// This is only correct to call on an offset for which `lookup_or_allocate` had just returned a
    /// non-null slot, and that slot was Empty and is still Empty.
    pub fn return_empty_slot(&mut self, offset: u64) {
        let node_offset = VmPageListNode::node_offset(offset);
        let index = VmPageListNode::node_index(offset);

        ltracef_level!(
            2,
            "{:p} offset {:#x} node_offset {:#x} index {}\n",
            core::ptr::from_ref(self),
            offset,
            node_offset,
            index
        );

        let cursor = Opaque::uninit();
        // lookup the tree node that holds this offset
        // SAFETY: `self.list.get()` is a valid initialized VmPageListBtree pointer, `cursor.get()`
        // is uninitialized storage.
        let node_ptr = unsafe {
            bindings::cpp_vm_page_list_btree_find(self.list.get(), node_offset, cursor.get())
        };
        debug_assert!(!node_ptr.is_null());

        // SAFETY: `node_ptr` is verified non-null by debug_assert and function contract.
        let node_ref: &mut VmPageListNode =
            unsafe { node_ptr.cast::<VmPageListNode>().as_mut_unchecked() };
        // check that the slot was empty
        debug_assert!(node_ref.lookup(index).is_empty());
        if node_ref.is_empty() {
            // node is empty, erase it.
            // SAFETY: `self.list.get()` is valid and `cursor` was initialized by `btree_find`.
            unsafe {
                bindings::cpp_vm_page_list_btree_erase_at(self.list.get(), cursor.get());
            }
        }
    }

    /// Removes any item at `offset` from the list and returns it, or `VmPageOrMarker::empty()` if
    /// none.
    pub fn remove_content(&mut self, offset: u64) -> VmPageOrMarker {
        let node_offset = VmPageListNode::node_offset(offset);
        let index = VmPageListNode::node_index(offset);

        ltracef_level!(
            2,
            "{:p} offset {:#x} node_offset {:#x} index {}\n",
            core::ptr::from_ref(self),
            offset,
            node_offset,
            index
        );

        let cursor = Opaque::uninit();
        // lookup the tree node that holds this page
        // SAFETY: `self.list.get()` is a valid initialized VmPageListBtree pointer, `cursor.get()`
        // is uninitialized storage.
        let node_ptr = unsafe {
            bindings::cpp_vm_page_list_btree_find(self.list.get(), node_offset, cursor.get())
        };
        if node_ptr.is_null() {
            return VmPageOrMarker::empty();
        }

        // SAFETY: `node_ptr` is verified non-null and points to a valid `VmPageListNode`.
        let node_ref: &mut VmPageListNode =
            unsafe { node_ptr.cast::<VmPageListNode>().as_mut_unchecked() };
        // free this page
        let page = node_ref.lookup_mut(index).swap(VmPageOrMarker::empty());
        if !page.is_empty() && node_ref.is_empty() {
            // if it was the last item in the node, remove the node from the tree
            ltracef_level!(2, "{:p} freeing the list node\n", core::ptr::from_ref(self));
            // SAFETY: `self.list.get()` is valid and `cursor` was initialized by `btree_find`.
            unsafe {
                bindings::cpp_vm_page_list_btree_erase_at(self.list.get(), cursor.get());
            }
        }
        page
    }

    /// Internal helper used when checking whether the offset falls in an interval.
    ///
    /// Determines whether `offset` falls in an interval involving `node` (which was returned by a
    /// lower-bound lookup). Returns any interval sentinel found, or `None`.
    fn is_offset_in_interval_helper(
        offset: u64,
        node_offset: u64,
        node: &VmPageListNode,
    ) -> Option<&VmPageOrMarker> {
        if offset < node_offset {
            node.node_starts_in_interval()
        } else {
            node.is_offset_in_interval(node_offset, offset)
        }
    }

    /// Internal helper to look up a node via `lower_bound` and check if `offset` falls in an
    /// interval. Returns any interval sentinel found, or `None`.
    fn find_interval_at_offset(&self, offset: u64) -> Option<&VmPageOrMarker> {
        // Find the node that would contain this offset.
        let node_offset = VmPageListNode::node_offset(offset);
        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // If the node containing offset is populated, the lower bound will return that node. If not
        // populated, it will return the next populated node.
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let entry = unsafe {
            bindings::cpp_vm_page_list_btree_lower_bound(self.list.get(), node_offset, cursor.get())
        };
        // Could not find a valid node >= node_offset. So offset cannot be part of an interval, an
        // interval would have an end slot.
        if entry.node.is_null() {
            return None;
        }
        // The page list shouldn't have any empty nodes.
        // SAFETY: `entry.node` is non-null and valid for lifetime of `&self`.
        let node = unsafe { entry.node.cast::<VmPageListNode>().as_ref_unchecked() };
        debug_assert!(!node.is_empty());
        // Check if offset is in an interval also querying the associated sentinel.
        let interval = Self::is_offset_in_interval_helper(offset, entry.offset, node);
        debug_assert!(interval.is_none_or(|p| p.is_interval()));
        interval
    }

    /// Returns true if the specified offset falls in a sparse zero interval.
    pub fn is_offset_in_zero_interval(&self, offset: u64) -> bool {
        self.find_interval_at_offset(offset).is_some_and(|p| p.is_interval_zero())
    }

    /// Returns true if the specified offset falls in a sparse page interval.
    fn is_offset_in_interval(&self, offset: u64) -> bool {
        self.find_interval_at_offset(offset).is_some()
    }

    /// Internal helper to return the start of an interval given the end along with the
    /// corresponding offset.
    fn find_interval_start_for_end(&self, end_offset: u64) -> (&VmPageOrMarker, u64) {
        // Find the node that would contain the end offset.
        let node_offset = VmPageListNode::node_offset(end_offset);
        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let node_ptr = unsafe {
            bindings::cpp_vm_page_list_btree_find(self.list.get(), node_offset, cursor.get())
        };
        debug_assert!(!node_ptr.is_null());
        // SAFETY: `node_ptr` is non-null and points to a valid `VmPageListNode`.
        let node = unsafe { node_ptr.cast::<VmPageListNode>().as_ref_unchecked() };
        let node_index = VmPageListNode::node_index(end_offset);
        debug_assert!(node.lookup(node_index).is_interval_end());

        // The only populated slots in an interval are the start and the end. So the interval start
        // will either be in the same node as the interval end, or the previous populated node to
        // the left.
        for idx in (0..node_index).rev() {
            let slot = node.lookup(idx);
            if !slot.is_empty() {
                debug_assert!(slot.is_interval_start());
                return (slot, node_offset + (idx as u64) * (page::SIZE as u64));
            }
        }

        // We could not find the start in the same node. Check the previous one.
        // SAFETY: `cursor.get()` is initialized.
        let prev_entry = unsafe { bindings::cpp_vm_page_list_btree_cursor_prev(cursor.get()) };
        debug_assert!(!prev_entry.node.is_null());
        // SAFETY: `prev_entry.node` is non-null and points to a valid `VmPageListNode`.
        let prev_node = unsafe { prev_entry.node.cast::<VmPageListNode>().as_ref_unchecked() };
        for idx in (0..VmPageListNode::PAGE_FAN_OUT).rev() {
            let slot = prev_node.lookup(idx);
            if !slot.is_empty() {
                debug_assert!(slot.is_interval_start());
                return (slot, prev_entry.offset + (idx as u64) * (page::SIZE as u64));
            }
        }

        // Should not reach here.
        unreachable!("sparse page interval is missing start sentinel");
    }

    /// Internal helper to return the end of an interval given the start along with the
    /// corresponding offset.
    fn find_interval_end_for_start(&self, start_offset: u64) -> (&VmPageOrMarker, u64) {
        // Find the node that would contain the start offset.
        let node_offset = VmPageListNode::node_offset(start_offset);
        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let node_ptr = unsafe {
            bindings::cpp_vm_page_list_btree_find(self.list.get(), node_offset, cursor.get())
        };
        debug_assert!(!node_ptr.is_null());
        // SAFETY: `node_ptr` is non-null and points to a valid `VmPageListNode`.
        let node = unsafe { node_ptr.cast::<VmPageListNode>().as_ref_unchecked() };
        let node_index = VmPageListNode::node_index(start_offset);
        debug_assert!(node.lookup(node_index).is_interval_start());

        // The only populated slots in an interval are the start and the end. So the interval end
        // will either be in the same node as the interval start, or the next populated node to the
        // right.
        for idx in (node_index + 1)..VmPageListNode::PAGE_FAN_OUT {
            let slot = node.lookup(idx);
            if !slot.is_empty() {
                debug_assert!(slot.is_interval_end());
                return (slot, node_offset + (idx as u64) * (page::SIZE as u64));
            }
        }

        // We could not find the end in the same node. Check the next one.
        // SAFETY: `cursor.get()` is initialized. Advance past current node and get next entry.
        let _ = unsafe { bindings::cpp_vm_page_list_btree_cursor_next(cursor.get()) };
        let next_entry = unsafe { bindings::cpp_vm_page_list_btree_cursor_get(cursor.get()) };
        debug_assert!(!next_entry.node.is_null());
        // SAFETY: `next_entry.node` is non-null and points to a valid `VmPageListNode`.
        let next_node = unsafe { next_entry.node.cast::<VmPageListNode>().as_ref_unchecked() };
        for idx in 0..VmPageListNode::PAGE_FAN_OUT {
            let slot = next_node.lookup(idx);
            if !slot.is_empty() {
                debug_assert!(slot.is_interval_end());
                return (slot, next_entry.offset + (idx as u64) * (page::SIZE as u64));
            }
        }

        // Should not reach here.
        unreachable!("sparse page interval is missing end sentinel");
    }

    /// Add a sparse zero interval spanning the range [start_offset, end_offset) with the specified
    /// dirty_state. The specified range must be previously unpopulated. This will try to merge the
    /// new zero interval with existing intervals to the left and/or right, if the dirty_state
    /// allows it.
    pub fn add_zero_interval(
        &mut self,
        start_offset: u64,
        end_offset: u64,
        dirty_state: ZeroRangeDirtyState,
    ) -> Result<(), Status> {
        self.add_zero_interval_internal(start_offset, end_offset, dirty_state, 0, false)
    }

    /// Helper to look up a slot at an offset and return a mutable VmPageOrMarker pointer. Only
    /// finds an existing slot and does not perform any allocations.
    fn lookup_slot(&mut self, offset: u64) -> *mut VmPageOrMarker {
        let node_offset = VmPageListNode::node_offset(offset);
        let index = VmPageListNode::node_index(offset);
        // SAFETY: `self.list.get()` is a valid pointer. Passing null cursor finds without
        // populating an out_cursor.
        let node_ptr = unsafe {
            bindings::cpp_vm_page_list_btree_find(
                self.list.get(),
                node_offset,
                core::ptr::null_mut(),
            )
        };
        if node_ptr.is_null() {
            core::ptr::null_mut()
        } else {
            debug_assert!(index < VmPageListNode::PAGE_FAN_OUT);
            // SAFETY: `node_ptr` is verified non-null and points to a valid `VmPageListNode`.
            let node_ptr = node_ptr.cast::<VmPageListNode>();
            let pages_ptr =
                unsafe { core::ptr::addr_of_mut!((*node_ptr).pages).cast::<VmPageOrMarker>() };
            unsafe { pages_ptr.add(index) }
        }
    }

    /// Internal helper for AddZeroInterval.
    /// |replace_existing_slot| can optionally be set to true if a zero interval spanning a single
    /// page is being added, and the slot at that offset is already populated (but Empty) and can be
    /// reused.
    fn add_zero_interval_internal(
        &mut self,
        start_offset: u64,
        end_offset: u64,
        dirty_state: ZeroRangeDirtyState,
        awaiting_clean_len: u64,
        replace_existing_slot: bool,
    ) -> Result<(), Status> {
        debug_assert!(start_offset.is_multiple_of(page::SIZE as u64));
        debug_assert!(end_offset.is_multiple_of(page::SIZE as u64));
        debug_assert!(start_offset < end_offset);
        debug_assert!(!replace_existing_slot || end_offset == start_offset + page::SIZE as u64);
        // If replace_existing_slot is true, then we might have the slot in an empty node, which is
        // not expected by any kind of page list traversal. So we cannot safely walk the specified
        // range. Instead, we will assert later that the slot being replaced is indeed empty. If we
        // don't end up using the slot, we will return the empty node.
        debug_assert!(
            replace_existing_slot
                || !self.any_pages_or_intervals_in_range(start_offset, end_offset)
        );
        debug_assert!(awaiting_clean_len == 0 || dirty_state == ZeroRangeDirtyState::Dirty);

        let page_size = page::SIZE as u64;
        let interval_start = start_offset;
        let interval_end = end_offset - page_size;
        let prev_offset = if interval_start > 0 { interval_start - page_size } else { 0 };
        let next_offset = interval_end + page_size;

        // Check if we can merge this zero interval with a preceding one.
        let mut merge_with_prev = false;
        let mut prev_slot: *mut VmPageOrMarker = core::ptr::null_mut();
        // The final start slot and offset after the merge. Used for awaiting_clean_length updates.
        let mut final_start: *mut VmPageOrMarker = core::ptr::null_mut();
        let mut final_start_offset: u64 = 0;
        if interval_start > 0 {
            prev_slot = self.lookup_slot(prev_offset);
            // We can merge to the left if we find a zero interval end or slot, and the dirty state
            // matches.
            // SAFETY: `prev_slot` if non-null points to a valid slot in a node in `self.list`.
            if !prev_slot.is_null()
                && unsafe { (*prev_slot).is_interval_zero() }
                && unsafe { (*prev_slot).is_interval_end() || (*prev_slot).is_interval_slot() }
                && unsafe { (*prev_slot).zero_interval_dirty_state() == dirty_state }
            {
                merge_with_prev = true;

                // Later we will also try to merge the new awaiting_clean_len into the interval
                // with which we're merging on the left. So stash the start sentinel for that
                // update later. We need to compute this before we start making changes to the
                // page list.
                if awaiting_clean_len > 0 {
                    if unsafe { (*prev_slot).is_interval_slot() } {
                        final_start = prev_slot;
                        final_start_offset = prev_offset;
                    } else {
                        let (_, off) = self.find_interval_start_for_end(prev_offset);
                        final_start_offset = off;
                        // This redundant lookup is so we can get a mutable reference to update
                        // the awaiting_clean_length. It is fine to be inefficient here as this
                        // case (i.e. awaiting_clean_len > 0) is unlikely.
                        final_start = self.lookup_slot(final_start_offset);
                    }
                }
            }
        }

        // Check if we can merge this zero interval with a following one.
        let mut merge_with_next = false;
        let next_slot = self.lookup_slot(next_offset);
        // We can merge to the right if we find a zero interval start or slot, and the dirty
        // state matches.
        // SAFETY: `next_slot` if non-null points to a valid slot in a node in `self.list`.
        if !next_slot.is_null()
            && unsafe { (*next_slot).is_interval_zero() }
            && unsafe { (*next_slot).is_interval_start() || (*next_slot).is_interval_slot() }
            && unsafe { (*next_slot).zero_interval_dirty_state() == dirty_state }
        {
            merge_with_next = true;
        }

        // First allocate any slots that might be needed to insert the interval.
        let mut new_start: *mut VmPageOrMarker = core::ptr::null_mut();
        let mut new_end: *mut VmPageOrMarker = core::ptr::null_mut();
        // If we could not merge with an interval to the left, we're going to need a new start
        // sentinel.
        if !merge_with_prev {
            new_start = match self.lookup_or_allocate_internal(interval_start) {
                Some(slot) => slot as *mut VmPageOrMarker,
                None => {
                    debug_assert!(!replace_existing_slot);
                    return Err(Status::NO_MEMORY);
                }
            };
            // SAFETY: `new_start` is non-null.
            debug_assert!(unsafe { (*new_start).is_empty() });
        }
        // If we could not merge with an interval to the right, we're going to need a new end
        // sentinel.
        if !merge_with_next {
            new_end = match self.lookup_or_allocate_internal(interval_end) {
                Some(slot) => slot as *mut VmPageOrMarker,
                None => {
                    debug_assert!(!replace_existing_slot);
                    // Clean up any slot we allocated for new_start before returning.
                    if !new_start.is_null() {
                        // SAFETY: `new_start` is non-null and empty.
                        debug_assert!(unsafe { (*new_start).is_empty() });
                        self.return_empty_slot(interval_start);
                    }
                    return Err(Status::NO_MEMORY);
                }
            };
            // SAFETY: `new_end` is non-null.
            debug_assert!(unsafe { (*new_end).is_empty() });
        }
        // If we were replacing an existing slot, but are able to merge both to the left and the
        // right, we won't need the slot anymore. So return it. Note that this is not strictly
        // needed but we want to be explicit for clarity. We know that the existing slot is in the
        // same node as the previous or the next slot (or both). We will either end up freeing one
        // or both of those slots, or retaining one or both of them. So the node the existing slot
        // shares with those slots will either be freed, or won't need freeing.
        if replace_existing_slot && merge_with_prev && merge_with_next {
            self.return_empty_slot(interval_start);
        }

        // Now that we've checked for all error conditions perform the actual update.
        if merge_with_prev {
            // Try to merge the new awaiting_clean_len into the previous interval.
            if awaiting_clean_len > 0 {
                // SAFETY: `final_start` is non-null when `awaiting_clean_len > 0` and
                // `merge_with_prev`.
                let final_start_ref = unsafe { &mut *final_start };
                let old_len = final_start_ref.zero_interval_awaiting_clean_length();
                // Can only merge the new awaiting_clean_len if there is no gap between the range
                // described by final_start's awaiting_clean_length and the start of the new
                // interval we're trying to add.
                if final_start_offset + old_len >= interval_start {
                    final_start_ref.set_zero_interval_awaiting_clean_length(
                        core::cmp::max(
                            final_start_offset + old_len,
                            interval_start + awaiting_clean_len,
                        ) - final_start_offset,
                    );
                }
            }
            // SAFETY: `prev_slot` is non-null when `merge_with_prev`.
            let prev_slot_ref = unsafe { &mut *prev_slot };
            if prev_slot_ref.is_interval_end() {
                // If the prev_slot was an interval end, we can simply extend that interval to
                // include the new interval. Free up the old interval end.
                *prev_slot_ref = VmPageOrMarker::empty();
            } else {
                // If the prev_slot was interval slot, we can extend the interval in that case too.
                // Change the old interval slot into an interval start.
                debug_assert!(prev_slot_ref.is_interval_slot());
                debug_assert!(prev_slot_ref.zero_interval_dirty_state() == dirty_state);
                prev_slot_ref.change_interval_sentinel(SentinelType::Start);
            }
        } else {
            // We could not merge with an interval to the left. Start a new interval.
            // SAFETY: `new_start` is non-null when `!merge_with_prev`.
            let new_start_ref = unsafe { &mut *new_start };
            debug_assert!(new_start_ref.is_empty());
            *new_start_ref = VmPageOrMarker::zero_interval(SentinelType::Start, dirty_state);
            if awaiting_clean_len > 0 {
                final_start = new_start;
                final_start_offset = interval_start;
                new_start_ref.set_zero_interval_awaiting_clean_length(awaiting_clean_len);
            }
        }

        if merge_with_next {
            // First see if we can merge the awaiting_clean_len of the interval we're merging with
            // the interval we have constructed so far on the left.
            // Note that it might still be possible to merge the awaiting_clean_len's of the left
            // and right intervals even if the specified awaiting_clean_len is 0, due to the
            // interval being inserted in the middle bridging the gap. We choose to skip that case
            // however to keep things more efficient; we don't want to needlessly lookup the
            // final_start unless we're also updating awaiting_clean_len for the interval being
            // added. Instead we choose to lose the awaiting_clean_len for the interval on the
            // right - this is also consistent with not being able to retain the awaiting_clean_len
            // if an interval is simply extended on the left.
            if awaiting_clean_len > 0 {
                // SAFETY: `next_slot` and `final_start` are non-null when `awaiting_clean_len > 0`.
                let next_slot_ref = unsafe { &mut *next_slot };
                let final_start_ref = unsafe { &mut *final_start };
                let len = next_slot_ref.zero_interval_awaiting_clean_length();
                let old_len = final_start_ref.zero_interval_awaiting_clean_length();
                // Can only merge the new awaiting_clean_len if there is no gap between the range
                // described by final_start's awaiting_clean_length and the start of the next
                // interval.
                if len > 0 && final_start_offset + old_len >= next_offset {
                    final_start_ref.set_zero_interval_awaiting_clean_length(
                        core::cmp::max(final_start_offset + old_len, next_offset + len)
                            - final_start_offset,
                    );
                }
            }

            // SAFETY: `next_slot` is non-null when `merge_with_next`.
            let next_slot_ref = unsafe { &mut *next_slot };
            if next_slot_ref.is_interval_start() {
                // If the next_slot was an interval start, we can move back the start to include the
                // new interval. Free up the old start.
                *next_slot_ref = VmPageOrMarker::empty();
            } else {
                // If the next_slot was an interval slot, we can move back the start in that case
                // too. Change the old interval slot into an interval end.
                debug_assert!(next_slot_ref.is_interval_slot());
                debug_assert!(next_slot_ref.zero_interval_dirty_state() == dirty_state);
                next_slot_ref.set_zero_interval_awaiting_clean_length(0);
                next_slot_ref.change_interval_sentinel(SentinelType::End);
            }
        } else {
            // We could not merge with an interval to the right. Install an interval end sentinel.
            // If the new zero interval spans a single page, we will already have installed a start
            // above, so change it to a slot sentinel.
            // SAFETY: `new_end` is non-null when `!merge_with_next`.
            let end_slot_ref = unsafe { &mut *new_end };
            if end_slot_ref.is_interval_start() {
                debug_assert!(end_slot_ref.zero_interval_dirty_state() == dirty_state);
                end_slot_ref.change_interval_sentinel(SentinelType::Slot);
            } else {
                debug_assert!(end_slot_ref.is_empty());
                *end_slot_ref = VmPageOrMarker::zero_interval(SentinelType::End, dirty_state);
            }
        }

        // If we ended up removing the prev_slot or next_slot, return the now empty slots.
        // SAFETY: `prev_slot` and `next_slot` are non-null when their respective merge is true.
        let return_prev_slot = merge_with_prev && unsafe { (*prev_slot).is_empty() };
        let mut return_next_slot = merge_with_next && unsafe { (*next_slot).is_empty() };
        if return_prev_slot {
            self.return_empty_slot(prev_offset);
            // next_slot and prev_slot could have come from the same node, in which case we've
            // already freed up the node containing next_slot when returning prev_slot.
            if return_next_slot
                && VmPageListNode::node_offset(prev_offset)
                    == VmPageListNode::node_offset(next_offset)
            {
                return_next_slot = false;
            }
        }
        if return_next_slot {
            debug_assert!(
                !return_prev_slot
                    || VmPageListNode::node_offset(prev_offset)
                        != VmPageListNode::node_offset(next_offset)
            );
            self.return_empty_slot(next_offset);
        }

        Ok(())
    }

    /// Populates individual interval slots in the range [start_offset, end_offset) that falls
    /// inside a sparse interval. The intent of this function is to allow the caller to prepare the
    /// range for overwriting (replacing with pages) by populating the required slots upfront, so
    /// that slot lookup does not fail after this call. Essentially simulates interval splits
    /// (lookup_or_allocate_check_for_interval) for every offset in the specified range, but does so
    /// more efficiently, instead of having to search the tree repeatedly for every single offset.
    pub fn populate_slots_in_interval(
        &mut self,
        start_offset: u64,
        end_offset: u64,
    ) -> Result<(), Status> {
        debug_assert!(start_offset.is_multiple_of(page::SIZE as u64));
        debug_assert!(end_offset.is_multiple_of(page::SIZE as u64));
        debug_assert!(end_offset > start_offset);
        let page_size = page::SIZE as u64;
        // Change the end_offset to an inclusive offset for convenience.
        let end_offset = end_offset - page_size;

        if cfg!(debug_assertions) {
            // The start_offset and end_offset should lie in an interval.
            debug_assert!(self.is_offset_in_interval(start_offset));
            debug_assert!(self.is_offset_in_interval(end_offset));
            // All the remaining offsets should be empty and lie in the same interval. So we should
            // find no pages or gaps in the range:
            // [start_offset + page_size, end_offset - page_size].
            if start_offset + page_size < end_offset {
                let status = self.for_every_page_and_gap_in_range(
                    start_offset + page_size,
                    end_offset,
                    |_, _| Status::BAD_STATE,
                    |_, _| Status::BAD_STATE,
                );
                debug_assert!(status.is_ok());
            }
        }

        // First allocate slots at start_offset and end_offset, splitting the interval around them
        // if required. If any of the subsequent operations fail, we should return these interval
        // slots. This function should either be able to populate all the slots requested, or the
        // interval should be returned to its original state before the call.
        let (start_slot, is_start_in_interval) =
            self.lookup_or_allocate_check_for_interval(start_offset, true);
        if start_slot.is_none() {
            return Err(Status::NO_MEMORY);
        }
        debug_assert!(is_start_in_interval);
        let start_dirty_state = {
            let start_slot_ref = start_slot.unwrap();
            debug_assert!(start_slot_ref.is_interval_slot());
            start_slot_ref.zero_interval_dirty_state()
        };
        // If only asked to populate single slot, nothing more to do.
        if start_offset == end_offset {
            return Ok(());
        }

        let (end_slot, is_end_in_interval) =
            self.lookup_or_allocate_check_for_interval(end_offset, true);
        if end_slot.is_none() {
            // Return the start slot before returning.
            self.return_interval_slot(start_offset);
            return Err(Status::NO_MEMORY);
        }
        debug_assert!(is_end_in_interval);
        {
            let end_slot_ref = end_slot.unwrap();
            debug_assert!(end_slot_ref.is_interval_slot());
            // We only support zero intervals and the start and end dirty state should match.
            debug_assert!(start_dirty_state == end_slot_ref.zero_interval_dirty_state());
        }

        // If there are no more empty slots to consider between start and end, return early.
        if end_offset == start_offset + page_size {
            return Ok(());
        }

        // Now we need to walk all page offsets from start_offset to end_offset and convert them all
        // to interval slots. Before we can do that, we will first allocate any page list nodes
        // required in the middle. After splitting the interval around start_offset, we know that
        // the node containing |start_offset + page_size| will be populated in order for it to hold
        // the interval start sentinel at that offset. Similarly, we know that the node containing
        // |end_offset - page_size| will be populated. So all the unpopulated nodes (if any) will
        // lie between these two nodes.
        let first_node_offset = VmPageListNode::node_offset(start_offset + page_size);
        let first_node_index = VmPageListNode::node_index(start_offset + page_size);
        let last_node_offset = VmPageListNode::node_offset(end_offset - page_size);
        let last_node_index = VmPageListNode::node_index(end_offset - page_size);
        debug_assert!(last_node_offset >= first_node_offset);
        if last_node_offset > first_node_offset + VmPageListNode::NODE_SPAN_BYTES {
            let first_unpopulated = first_node_offset + VmPageListNode::NODE_SPAN_BYTES;
            let last_unpopulated = last_node_offset - VmPageListNode::NODE_SPAN_BYTES;
            let mut node_off = first_unpopulated;
            while node_off <= last_unpopulated {
                // SAFETY: `self.list.get()` is a valid pointer. Passing null cursor inserts
                // without a hint.
                let node_ptr = unsafe {
                    bindings::cpp_vm_page_list_btree_insert(
                        self.list.get(),
                        node_off,
                        core::ptr::null_mut(),
                    )
                };
                if node_ptr.is_null() {
                    // If allocating a new node fails, clean up all the new nodes we might have
                    // installed until this point, which is all the empty nodes starting at
                    // first_unpopulated to before the node that failed.
                    let mut cleanup_off = first_unpopulated;
                    while cleanup_off < node_off {
                        let cleanup_cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
                        // SAFETY: `self.list.get()` and `cleanup_cursor.get()` are valid pointers.
                        let cleanup_ptr = unsafe {
                            bindings::cpp_vm_page_list_btree_find(
                                self.list.get(),
                                cleanup_off,
                                cleanup_cursor.get(),
                            )
                        };
                        if !cleanup_ptr.is_null() {
                            // SAFETY: `self.list.get()` and `cleanup_cursor.get()` are valid.
                            unsafe {
                                bindings::cpp_vm_page_list_btree_erase_at(
                                    self.list.get(),
                                    cleanup_cursor.get(),
                                );
                            }
                        }
                        cleanup_off += VmPageListNode::NODE_SPAN_BYTES;
                    }
                    // Also return the start and end slots that we split above.
                    self.return_interval_slot(start_offset);
                    self.return_interval_slot(end_offset);
                    return Err(Status::NO_MEMORY);
                }
                node_off += VmPageListNode::NODE_SPAN_BYTES;
            }
        }

        // Now that all allocations have succeeded, we know that the rest of the operation cannot
        // fail. Walk all offsets after start_offset and before end_offset, overwriting all the
        // slots as interval slots.
        let mut node_offset = first_node_offset;
        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let node_ptr = unsafe {
            bindings::cpp_vm_page_list_btree_find(self.list.get(), node_offset, cursor.get())
        };
        // This has to emulate calls to lookup_or_allocate_check_for_interval for all slots in the
        // range, which includes retaining awaiting_clean_length. After the start_slot split, the
        // slot following it might contain a non-zero awaiting_clean_length for the interval
        // following it, this needs to be "shifted" to the slot after the last one we populate,
        // adjusting for all the populated slots we encounter in the middle.
        //
        // For example, if we were populating 3 slots starting at the interval start, whose
        // awaiting_clean_length was 5 pages, the awaiting_clean_lengths for the 3 slots and the
        // remaining interval at the end of the call should be (in pages): [1, 1, 1, 2]
        // If awaiting_clean_length had initially been 2, we would instead have: [1, 1, 0, 0]
        let mut awaiting_clean_len = {
            // SAFETY: `node_ptr` is non-null and valid.
            let first_node = unsafe { node_ptr.cast::<VmPageListNode>().as_ref_unchecked() };
            first_node.lookup(first_node_index).zero_interval_awaiting_clean_length()
        };

        while node_offset <= last_node_offset {
            // SAFETY: `cursor.get()` is a valid pointer.
            let entry = unsafe { bindings::cpp_vm_page_list_btree_cursor_get(cursor.get()) };
            debug_assert!(!entry.node.is_null());
            debug_assert_eq!(entry.offset, node_offset);
            // SAFETY: `entry.node` is non-null and valid.
            let node = unsafe { entry.node.cast::<VmPageListNode>().as_mut_unchecked() };
            let start_idx = if node_offset == first_node_offset { first_node_index } else { 0 };
            let end_idx = if node_offset == last_node_offset {
                last_node_index
            } else {
                VmPageListNode::PAGE_FAN_OUT - 1
            };
            for index in start_idx..=end_idx {
                let cur = node.lookup_mut(index);
                *cur = VmPageOrMarker::zero_interval(SentinelType::Slot, start_dirty_state);
                if awaiting_clean_len > 0 {
                    cur.set_zero_interval_awaiting_clean_length(page_size);
                    awaiting_clean_len -= page_size;
                }
            }
            // Advance cursor to next node.
            // SAFETY: `cursor.get()` is a valid pointer.
            let _ = unsafe { bindings::cpp_vm_page_list_btree_cursor_next(cursor.get()) };
            node_offset += VmPageListNode::NODE_SPAN_BYTES;
        }

        if awaiting_clean_len > 0 {
            // Set awaiting_clean_length for the last populated slot too.
            if let Some(mut end_ref) = self.lookup_mut(end_offset) {
                end_ref.set_zero_interval_awaiting_clean_length(page_size);
            }
            awaiting_clean_len -= page_size;
            // If there is still a remaining awaiting_clean_length, carry it over to the interval
            // next to the last slot, if there is one.
            if awaiting_clean_len > 0
                && let Some(mut next_ref) = self.lookup_mut(end_offset + page_size)
                && (next_ref.is_interval_start() || next_ref.is_interval_slot())
            {
                let old_len = next_ref.zero_interval_awaiting_clean_length();
                next_ref.set_zero_interval_awaiting_clean_length(core::cmp::max(
                    old_len,
                    awaiting_clean_len,
                ));
            }
        }

        if cfg!(debug_assertions) {
            // All offsets in the range [start_offset, end_offset] should contain interval slots.
            let mut next_off = start_offset;
            let status =
                self.for_every_page_in_range(start_offset, end_offset + page_size, |p, off| {
                    if off != next_off || !p.is_interval_slot() {
                        return Status::BAD_STATE;
                    }
                    next_off += page_size;
                    Status::NEXT
                });
            debug_assert!(status.is_ok());
        }

        Ok(())
    }

    /// Helper to return an unused interval slot so that it can be merged back into the interval it
    /// was populated/split from.
    pub fn return_interval_slot(&mut self, offset: u64) {
        // We should be able to lookup a pre-existing interval slot.
        let slot = self.lookup_or_allocate_internal(offset);
        debug_assert!(slot.is_some());
        let slot = slot.unwrap();
        debug_assert!(slot.is_interval_slot());

        // We only support zero intervals for now. If more interval types are added in the future,
        // handle them here.
        debug_assert!(slot.is_interval_zero());
        let dirty_state = slot.zero_interval_dirty_state();
        let awaiting_clean_len = slot.zero_interval_awaiting_clean_length();
        // Temporarily empty the slot and then add a zero interval back in at the same spot using
        // add_zero_interval, which will ensure that the slot is merged to the left and/or right as
        // applicable. We don't need to return the empty slot here because we're asking
        // add_zero_interval_internal to reuse the existing slot.
        *slot = VmPageOrMarker::empty();
        let status = self.add_zero_interval_internal(
            offset,
            offset + page::SIZE as u64,
            dirty_state,
            awaiting_clean_len,
            true,
        );
        // We are reusing an existing slot, so we cannot fail with NO_MEMORY.
        debug_assert!(status.is_ok());
    }

    /// Walk the page tree, calling the passed in function on every tree node.
    pub fn for_every_page<F>(&self, per_page_func: F) -> Result<(), Status>
    where
        F: FnMut(&VmPageOrMarker, u64) -> Status,
    {
        self.for_every_page_in_range(0, Self::MAX_SIZE, per_page_func)
    }

    /// Similar to `for_every_page`, but the `per_page_func` gets called with a `VmPageOrMarkerRef`
    /// instead of a `&VmPageOrMarker`, allowing for limited mutation.
    pub fn for_every_page_ref<F>(&mut self, per_page_func: F) -> Result<(), Status>
    where
        F: FnMut(VmPageOrMarkerRef<'_>, u64) -> Status,
    {
        self.for_every_page_in_range_ref(0, Self::MAX_SIZE, per_page_func)
    }

    /// Similar to `for_every_page`, but the `per_page_func` gets called with a
    /// `&mut VmPageOrMarker`, allowing for mutation.
    pub fn for_every_page_mut<F>(&mut self, per_page_func: F) -> Result<(), Status>
    where
        F: FnMut(&mut VmPageOrMarker, u64) -> Status,
    {
        self.for_every_page_in_range_mut(0, Self::MAX_SIZE, per_page_func)
    }

    /// Internal helper for immutable range traversals.
    ///
    /// Calls the provided callback for every page in the given range.
    fn for_every_page_in_range_internal<F>(
        &self,
        cursor: &Opaque<bindings::VmPageListBtreeCursor>,
        start_offset: u64,
        end_offset: u64,
        mut per_page_func: F,
    ) -> Status
    where
        F: FnMut(&VmPageOrMarker, u64) -> Status,
    {
        debug_assert!(start_offset.is_multiple_of(page::SIZE as u64));
        debug_assert!(end_offset.is_multiple_of(page::SIZE as u64));

        loop {
            // SAFETY: `cursor.get()` is a valid pointer.
            let entry = unsafe { bindings::cpp_vm_page_list_btree_cursor_next(cursor.get()) };
            if entry.node.is_null() || entry.offset >= end_offset {
                break;
            }
            // SAFETY: `entry.node` is non-null and valid for the lifetime of `&self`.
            let node = unsafe { entry.node.cast::<VmPageListNode>().as_ref_unchecked() };
            let start = core::cmp::max(start_offset, entry.offset);
            let end = core::cmp::min(VmPageListNode::end_offset(entry.offset), end_offset);
            let status = node.for_every_page_in_range(entry.offset, start, end, &mut per_page_func);
            if status != Status::NEXT {
                return status;
            }
        }

        Status::NEXT
    }

    /// Walk the page tree, calling the passed in function on every tree node in the given range.
    pub fn for_every_page_in_range<F>(
        &self,
        start_offset: u64,
        end_offset: u64,
        per_page_func: F,
    ) -> Result<(), Status>
    where
        F: FnMut(&VmPageOrMarker, u64) -> Status,
    {
        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // Position cursor at lower bound.
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        unsafe {
            bindings::cpp_vm_page_list_btree_lower_bound(
                self.list.get(),
                VmPageListNode::node_offset(start_offset),
                cursor.get(),
            );
        }
        let status =
            self.for_every_page_in_range_internal(&cursor, start_offset, end_offset, per_page_func);
        if status != Status::NEXT {
            if status == Status::STOP {
                return Ok(());
            }
            return Err(status);
        }
        Ok(())
    }

    /// Iterates over all pages and gaps in the specified range `[start_offset, end_offset)`.
    ///
    /// `per_page_func` is called for every page/ref/marker or interval sentinel in the range with
    /// `(&VmPageOrMarker, offset)`.
    /// `per_gap_func` is called for every gap (unpopulated range) with `(gap_start, gap_end)`.
    pub fn for_every_page_and_gap_in_range<PageFunc, GapFunc>(
        &self,
        start_offset: u64,
        end_offset: u64,
        mut per_page_func: PageFunc,
        mut per_gap_func: GapFunc,
    ) -> Result<(), Status>
    where
        PageFunc: FnMut(&VmPageOrMarker, u64) -> Status,
        GapFunc: FnMut(u64, u64) -> Status,
    {
        let page_size = page::SIZE as u64;
        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // Position cursor at lower bound.
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let first_entry = unsafe {
            bindings::cpp_vm_page_list_btree_lower_bound(
                self.list.get(),
                VmPageListNode::node_offset(start_offset),
                cursor.get(),
            )
        };

        let mut expected_next_off = start_offset;
        // Set to true when we encounter an interval start but haven't yet encountered the end.
        let mut in_interval = false;

        let status =
            self.for_every_page_in_range_internal(&cursor, start_offset, end_offset, |p, off| {
                // Update our interval tracking first. Should the callbacks later request an early
                // exit then this work is wasted, but doing it first, and unconditionally, lets the
                // compiler perform better common expression elimination with the per_gap_func check
                // next.
                if p.is_interval_start() {
                    // We should not already have been tracking an interval.
                    debug_assert!(!in_interval);
                    // Start and end sentinel interval types should match. Since we only support
                    // zero intervals currently, we can simply check for that.
                    debug_assert!(p.is_interval_zero());
                    in_interval = true;
                } else if p.is_interval_end() {
                    // If this is not the first populated slot we encountered, we should have been
                    // tracking a valid interval.
                    debug_assert!(in_interval || expected_next_off == start_offset);
                    // Start and end sentinel interval types should match. Since we only support
                    // zero intervals currently, we can simply check for that.
                    debug_assert!(p.is_interval_zero());
                    // Reset interval tracking.
                    in_interval = false;
                }

                let mut status = Status::NEXT;
                // We can move ahead of expected_next_off in the case of an interval too, which
                // represents a run of pages. Make sure this is not an interval before calling the
                // per_gap_func.
                if expected_next_off != off && !p.is_interval_end() {
                    status = per_gap_func(expected_next_off, off);
                }
                expected_next_off = off + page_size;
                if status == Status::NEXT {
                    status = per_page_func(p, off);
                }
                status
            });

        if status != Status::NEXT {
            if status == Status::STOP {
                return Ok(());
            }
            return Err(status);
        }

        // Handle the last gap after checking that we are not in an interval. Note that simply
        // checking for in_interval is not sufficient, as it is possible to have started the
        // traversal partway into an interval, in which case we would not have seen the interval
        // start and in_interval would be false. So we perform a quick check for in_interval first
        // and if that fails perform the more expensive IsOffsetInInterval() check. The
        // IsOffsetInInterval() call is further gated by whether we encountered any page at all in
        // the traversal above. If we saw at least one page in the traversal, we know that we could
        // not be in an interval without in_interval being true because we would have seen the
        // interval start.
        if expected_next_off != end_offset {
            // Traversal ended in an interval if in_interval was true, OR if the traversal did not
            // see any page at all and the start_offset is in an interval (Note that in this latter
            // case all offsets in the range [start_offset, end_offset) would lie in the same
            // interval, so we can just check one of them).
            let ended_in_interval = in_interval
                || (expected_next_off == start_offset
                    && !first_entry.node.is_null()
                    && Self::is_offset_in_interval_helper(
                        start_offset,
                        first_entry.offset,
                        // SAFETY: `first_entry.node` is non-null.
                        unsafe { first_entry.node.cast::<VmPageListNode>().as_ref_unchecked() },
                    )
                    .is_some());
            if !ended_in_interval {
                let status = per_gap_func(expected_next_off, end_offset);
                if status != Status::NEXT && status != Status::STOP {
                    return Err(status);
                }
            }
        }

        Ok(())
    }

    /// Similar to `for_every_page_in_range`, but the `per_page_func` gets called with a
    /// `VmPageOrMarkerRef` instead of a `&VmPageOrMarker`, allowing for limited mutation.
    pub fn for_every_page_in_range_ref<F>(
        &mut self,
        start_offset: u64,
        end_offset: u64,
        mut per_page_func: F,
    ) -> Result<(), Status>
    where
        F: FnMut(VmPageOrMarkerRef<'_>, u64) -> Status,
    {
        self.for_every_page_in_range_mut(start_offset, end_offset, |slot, offset| {
            per_page_func(VmPageOrMarkerRef::new(slot), offset)
        })
    }

    /// Similar to `for_every_page_in_range`, but the `per_page_func` gets called with a
    /// `&mut VmPageOrMarker`, allowing for mutation.
    pub fn for_every_page_in_range_mut<F>(
        &mut self,
        start_offset: u64,
        end_offset: u64,
        per_page_func: F,
    ) -> Result<(), Status>
    where
        F: FnMut(&mut VmPageOrMarker, u64) -> Status,
    {
        self.for_every_page_in_range_internal_mut::<SkipNodeCheck, _>(
            start_offset,
            end_offset,
            per_page_func,
        )
    }

    /// Internal helper for mutable range traversals with optional empty-node cleanup.
    ///
    /// Calls the provided callback for every page in the given range. If `N::CLEANUP_EMPTY` is
    /// true, then it is assumed the `per_page_func` may remove pages and page nodes will be checked
    /// to see if they are empty and can be cleaned up.
    fn for_every_page_in_range_internal_mut<N: NodeCheck, F>(
        &mut self,
        start_offset: u64,
        end_offset: u64,
        mut per_page_func: F,
    ) -> Result<(), Status>
    where
        F: FnMut(&mut VmPageOrMarker, u64) -> Status,
    {
        debug_assert!(start_offset.is_multiple_of(page::SIZE as u64));
        debug_assert!(end_offset.is_multiple_of(page::SIZE as u64));

        let cursor = Opaque::<bindings::VmPageListBtreeCursor>::uninit();
        // SAFETY: `self.list.get()` and `cursor.get()` are valid pointers.
        let mut entry = unsafe {
            bindings::cpp_vm_page_list_btree_lower_bound(
                self.list.get(),
                VmPageListNode::node_offset(start_offset),
                cursor.get(),
            )
        };
        while !entry.node.is_null() {
            if entry.offset >= end_offset {
                break;
            }
            // SAFETY: `entry.node` is non-null and valid for the node's scope.
            let node = unsafe { entry.node.cast::<VmPageListNode>().as_mut_unchecked() };
            let start = core::cmp::max(start_offset, entry.offset);
            let end = core::cmp::min(VmPageListNode::end_offset(entry.offset), end_offset);
            let status =
                node.for_every_page_in_range_mut(entry.offset, start, end, &mut per_page_func);
            let is_empty = node.is_empty();
            if N::CLEANUP_EMPTY {
                if is_empty {
                    // SAFETY: `cursor` is positioned at this valid node.
                    // `cpp_vm_page_list_btree_erase_at` erases the node and updates `cursor`
                    // to point to the next node in the tree.
                    unsafe {
                        bindings::cpp_vm_page_list_btree_erase_at(self.list.get(), cursor.get());
                    }
                } else {
                    // SAFETY: `cursor.get()` is a valid pointer.
                    let _ = unsafe { bindings::cpp_vm_page_list_btree_cursor_next(cursor.get()) };
                }
            } else {
                // SAFETY: `cursor.get()` is a valid pointer.
                let _ = unsafe { bindings::cpp_vm_page_list_btree_cursor_next(cursor.get()) };
            }
            if status != Status::NEXT {
                return if status == Status::STOP { Ok(()) } else { Err(status) };
            }
            // Peek at the new node after step or erase.
            // SAFETY: `cursor.get()` is a valid pointer.
            entry = unsafe { bindings::cpp_vm_page_list_btree_cursor_get(cursor.get()) };
        }
        Ok(())
    }

    /// Calls the provided callback for every page or marker in the range
    /// [start_offset, end_offset). The callback can modify the `VmPageOrMarker` and take ownership
    /// of any pages, or leave them in place. The difference between this and `for_every_page` is
    /// that because this allows for modifying the underlying pages, any intermediate data
    /// structures can be checked and potentially freed if no longer needed.
    pub fn remove_pages<F>(
        &mut self,
        start_offset: u64,
        end_offset: u64,
        per_page_fn: F,
    ) -> Result<(), Status>
    where
        F: FnMut(&mut VmPageOrMarker, u64) -> Status,
    {
        self.for_every_page_in_range_internal_mut::<CleanupEmptyNodeCheck, _>(
            start_offset,
            end_offset,
            per_page_fn,
        )
    }

    /// Returns true if any pages (actual pages, references, or markers) are in the given range, or
    /// if the range forms a part of a sparse page interval.
    pub fn any_pages_or_intervals_in_range(&self, start_offset: u64, end_offset: u64) -> bool {
        let mut found_page = false;
        let _ = self.for_every_page_in_range(start_offset, end_offset, |_, _| {
            found_page = true;
            Status::STOP
        });
        // It is possible that the range forms a part of an interval even if no nodes in the range
        // have populated slots. We can determine that by checking to see if the start offset in
        // the range falls in an interval (we could technically perform this check for any inclusive
        // offset in the range since the range is entirely unpopulated and hence would only fall in
        // the same interval if applicable).
        if found_page { true } else { self.is_offset_in_interval(start_offset) }
    }

    /// Similar to `any_pages_or_intervals_in_range` but skips over any ParentContent markers, as
    /// these do not represent content owned by this page list, but rather content owned by a
    /// parent.
    pub fn any_owned_pages_or_intervals_in_range(
        &self,
        start_offset: u64,
        end_offset: u64,
    ) -> bool {
        let mut found_page = false;
        let _ = self.for_every_page_in_range(start_offset, end_offset, |page, _| {
            if page.is_parent_content() {
                return Status::NEXT;
            }
            found_page = true;
            Status::STOP
        });
        // It is possible that the range forms a part of an interval even if no nodes in the range
        // have populated slots. We can determine that by checking to see if the start offset in
        // the range falls in an interval (we could technically perform this check for any inclusive
        // offset in the range since the range is entirely unpopulated and hence would only fall in
        // the same interval if applicable).
        if found_page { true } else { self.is_offset_in_interval(start_offset) }
    }

    /// Release and call `free_content_fn` on every item in the page list. Gives `free_content_fn`
    /// ownership of the content. After calling this method, all slots in the page list are empty.
    pub fn remove_all_content<F>(&mut self, mut free_content_fn: F)
    where
        F: FnMut(VmPageOrMarker),
    {
        // walk the tree in order, freeing all the pages on every node
        for node in self.nodes_mut() {
            // per page get a reference to the page pointer inside the page list node
            for slot in &mut node.pages {
                if !slot.is_empty() {
                    free_content_fn(slot.swap(VmPageOrMarker::empty()));
                }
            }
        }
        // empty the tree
        self.clear();
    }
}

impl Default for VmPageList {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for VmPageList {
    fn drop(&mut self) {
        ltracef!("{:p}\n", core::ptr::from_ref(self));
        debug_assert!(
            self.has_no_page_ref_or_marker(),
            "VmPageList dropped with live pages, refs, or markers"
        );
        // SAFETY: `self.list.get()` is a valid initialized VmPageListBtree pointer to be destroyed.
        unsafe {
            bindings::cpp_vm_page_list_btree_destroy(self.list.get());
        }
    }
}

impl core::fmt::Debug for VmPageList {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VmPageList")
            .field("is_empty", &self.is_empty())
            .field("has_no_page_or_ref", &self.has_no_page_or_ref())
            .field("has_no_page_ref_or_marker", &self.has_no_page_ref_or_marker())
            .finish()
    }
}

/// Class which holds the list of vm_page structs removed from a VmPageList
/// by AddPagesFrom. The list include information about uncommitted pages and markers.
/// Every splice list is expected to go through the following series of states:
/// 1. The splice list is created.
/// 2. List is Initialized with the desired range.
/// 3. Pages are added to the splice list.
/// 4. The list is `Finalize`d, meaning that it can no longer be modified by `Append`.
/// 5. Pages are then `Pop`d from the list. Once all the pages are popped, the list is considered
///    "processed".
/// 6. The list is then considered `Processed` and can be destroyed.
#[pin_data(PinnedDrop)]
pub struct VmPageSpliceList {
    #[pin]
    opaque: Opaque<bindings::VmPageSpliceList>,
}

unsafe_pinned_drop_ffi!(VmPageSpliceList, bindings::cpp_vm_page_splice_list_destroy);

impl VmPageSpliceList {
    /// Returns an in-place initializer for stack-pinning a `VmPageSpliceList`.
    pub fn new() -> impl pin_init::PinInit<Self> {
        unsafe fn init_shim(ptr: *mut core::ffi::c_void) {
            let list_ptr: *mut bindings::VmPageSpliceList = ptr.cast();
            // SAFETY: `ptr` is guaranteed by `pin_init_ffi!` to point to valid `VmPageSpliceList`
            // storage.
            unsafe { bindings::cpp_vm_page_splice_list_construct(list_ptr) }
        }
        pin_init_ffi!(init_shim)
    }

    /// Returns true after the whole collection has been processed by Pop.
    pub fn is_processed(&self) -> bool {
        // SAFETY: `self.opaque` holds a valid `VmPageSpliceList`.
        unsafe { bindings::cpp_vm_page_splice_list_is_processed(self.opaque.get()) }
    }

    /// Returns a raw pointer to the underlying C++ `VmPageSpliceList`.
    ///
    /// Callers must not use the returned raw pointer to move the object in memory.
    pub fn as_raw(self: Pin<&mut Self>) -> *mut bindings::VmPageSpliceList {
        // SAFETY: Obtaining a raw pointer to `opaque` does not move the pinned object.
        unsafe { self.get_unchecked_mut().opaque.get() }
    }
}

#[cfg(ktest)]
#[unittest::suite]
/// Unit tests for VmPageOrMarker.
mod vm_page_list_rs {
    use super::{
        ReferenceValue, SentinelType, VmPageListNode, VmPageOrMarker, VmPageOrMarkerRef,
        ZeroRangeDirtyState,
    };
    use unittest::{expect_eq, expect_false, expect_true};

    /// Tests empty state creation and predicate checks.
    #[test]
    fn test_page_or_marker_empty() {
        let pm = VmPageOrMarker::empty();
        expect_true!(pm.is_empty());
        expect_false!(pm.is_page());
        expect_false!(pm.is_reference());
        expect_false!(pm.is_page_or_ref());
    }

    /// Tests release behavior on an empty VmPageOrMarker.
    #[test]
    fn test_page_or_marker_release() {
        let mut pm = VmPageOrMarker::empty();
        let raw = pm.release();
        expect_eq!(raw, VmPageOrMarker::PAGE_TYPE);
        expect_true!(pm.is_empty());
    }

    /// Tests swapping two empty VmPageOrMarker instances.
    #[test]
    fn test_page_or_marker_swap() {
        let mut pm1 = VmPageOrMarker::empty();
        let pm2 = VmPageOrMarker::empty();
        let prev = pm1.swap(pm2);
        expect_true!(prev.is_empty());
        expect_true!(pm1.is_empty());
    }

    /// Tests reference value creation, alignment mask, and getter properties.
    #[test]
    fn test_reference_value() {
        expect_eq!(ReferenceValue::ALIGN_BITS, 3);
        expect_eq!(ReferenceValue::ALIGN_MASK, 0b111);

        let ref_zero = ReferenceValue::new(0x0);
        expect_eq!(ref_zero.value(), 0x0);

        let ref_aligned = ReferenceValue::new(0x1000);
        expect_eq!(ref_aligned.value(), 0x1000);

        let ref_max = ReferenceValue::new(0xFFFFFFF8);
        expect_eq!(ref_max.value(), 0xFFFFFFF8);
    }

    /// Tests reference packing in VmPageOrMarker, predicates, and release behavior.
    #[test]
    fn test_page_or_marker_reference() {
        let ref_val = ReferenceValue::new(0x1000); // 8-byte aligned
        let mut pm = VmPageOrMarker::from_reference(ref_val);
        expect_true!(pm.is_reference());
        expect_false!(pm.is_page());
        expect_false!(pm.is_empty());
        expect_true!(pm.is_page_or_ref());

        expect_true!(pm.reference() == ref_val);
        expect_eq!(pm.reference().value(), ref_val.value());

        let released_ref = pm.release_reference();
        expect_true!(released_ref == ref_val);
        expect_true!(pm.is_empty());
    }

    /// Tests swapping references and swap_reference_for_reference method.
    #[test]
    fn test_page_or_marker_reference_swap() {
        let ref_val1 = ReferenceValue::new(0x1000);
        let ref_val2 = ReferenceValue::new(0x2000);
        let mut pm1 = VmPageOrMarker::from_reference(ref_val1);
        let pm2 = VmPageOrMarker::from_reference(ref_val2);

        // Test general swap of two reference instances
        let mut old = pm1.swap(pm2);
        expect_true!(old.reference() == ref_val1);
        expect_true!(pm1.reference() == ref_val2);

        // Test swap_reference_for_reference method directly
        let ref_val3 = ReferenceValue::new(0x3000);
        let old_ref = pm1.swap_reference_for_reference(ref_val3);
        expect_true!(old_ref == ref_val2);
        expect_true!(pm1.reference() == ref_val3);

        let _ = old.release_reference();
        let _ = pm1.release_reference();
    }

    /// Tests marker state creation and share count modifications.
    #[test]
    fn test_page_or_marker_marker() {
        let mut pm = VmPageOrMarker::marker();
        expect_true!(pm.is_marker());
        expect_false!(pm.is_page());
        expect_false!(pm.is_reference());
        expect_false!(pm.is_parent_content());
        expect_eq!(pm.marker_share_count(), 0);

        pm.increment_marker_share_count();
        expect_eq!(pm.marker_share_count(), 1);

        pm.increment_marker_share_count();
        expect_eq!(pm.marker_share_count(), 2);

        pm.decrement_marker_share_count();
        expect_eq!(pm.marker_share_count(), 1);

        pm.set_marker_share_count(100);
        expect_eq!(pm.marker_share_count(), 100);

        let pm_shared = VmPageOrMarker::marker_with_share_count(5);
        expect_true!(pm_shared.is_marker());
        expect_eq!(pm_shared.marker_share_count(), 5);
    }

    /// Tests parent content state properties.
    #[test]
    fn test_page_or_marker_parent_content() {
        let pm = VmPageOrMarker::parent_content();
        expect_true!(pm.is_parent_content());
        expect_false!(pm.is_page());
        expect_false!(pm.is_reference());
        expect_false!(pm.is_marker());
    }

    /// Tests ZeroRange basic value initialization and boundary limits.
    #[test]
    fn test_zero_range_basic() {
        let zr = ZeroRange::new(0);
        expect_eq!(zr.value(), 0);
        expect_eq!(zr.dirty_state(), ZeroRangeDirtyState::Untracked);
        expect_eq!(zr.awaiting_clean_length(), 0);

        let zr2 = ZeroRange::new(0x80); // 128 (aligned to 7 bits)
        expect_eq!(zr2.value(), 0x80);
    }

    /// Tests ZeroRange dirty state transitions.
    #[test]
    fn test_zero_range_dirty_state() {
        let mut zr = ZeroRange::new(0);
        expect_eq!(zr.dirty_state(), ZeroRangeDirtyState::Untracked);

        zr.set_dirty_state(ZeroRangeDirtyState::Dirty);
        expect_eq!(zr.dirty_state(), ZeroRangeDirtyState::Dirty);

        zr.set_dirty_state(ZeroRangeDirtyState::Untracked);
        expect_eq!(zr.dirty_state(), ZeroRangeDirtyState::Untracked);
    }

    /// Tests ZeroRange awaiting clean length encoding.
    #[test]
    fn test_zero_range_awaiting_clean_length() {
        let mut zr = ZeroRange::new(0);
        // Can only set length if state is Dirty.
        zr.set_dirty_state(ZeroRangeDirtyState::Dirty);
        expect_eq!(zr.awaiting_clean_length(), 0);

        // awaiting_clean_length is page aligned (4096).
        zr.set_awaiting_clean_length(4096);
        expect_eq!(zr.awaiting_clean_length(), 4096);

        zr.set_awaiting_clean_length(8192);
        expect_eq!(zr.awaiting_clean_length(), 8192);

        zr.set_awaiting_clean_length(0);
        expect_eq!(zr.awaiting_clean_length(), 0);
    }

    /// Tests interval creation, properties, sentinel transitions, and dirty states.
    #[test]
    fn test_page_or_marker_interval() {
        let mut pm =
            VmPageOrMarker::zero_interval(SentinelType::Slot, ZeroRangeDirtyState::Untracked);
        expect_true!(pm.is_interval());
        expect_true!(pm.is_interval_zero());
        expect_true!(pm.is_interval_slot());
        expect_false!(pm.is_interval_start());
        expect_false!(pm.is_interval_end());
        expect_true!(pm.is_zero_interval_untracked());
        expect_false!(pm.is_zero_interval_clean());
        expect_false!(pm.is_zero_interval_dirty());

        pm.change_interval_sentinel(SentinelType::Start);
        expect_true!(pm.is_interval_start());
        expect_false!(pm.is_interval_slot());

        let mut pm_dirty =
            VmPageOrMarker::zero_interval(SentinelType::Start, ZeroRangeDirtyState::Dirty);
        expect_true!(pm_dirty.is_zero_interval_dirty());
        expect_eq!(pm_dirty.zero_interval_awaiting_clean_length(), 0);

        pm_dirty.set_zero_interval_awaiting_clean_length(8192);
        expect_eq!(pm_dirty.zero_interval_awaiting_clean_length(), 8192);
    }

    /// Tests VmPageOrMarkerRef deref, mutations, and swap_content.
    #[test]
    fn test_page_or_marker_ref() {
        let mut marker = VmPageOrMarker::marker();
        {
            let mut marker_ref = VmPageOrMarkerRef::new(&mut marker);
            expect_true!(marker_ref.is_marker());
            marker_ref.increment_marker_share_count();
            expect_eq!(marker_ref.marker_share_count(), 1);
            marker_ref.decrement_marker_share_count();
            expect_eq!(marker_ref.marker_share_count(), 0);
        }

        let mut slot = VmPageOrMarker::from_reference(ReferenceValue::new(0x1000));
        {
            let mut slot_ref = VmPageOrMarkerRef::new(&mut slot);
            let prev_ref = slot_ref.swap_reference_for_reference(ReferenceValue::new(0x2000));
            expect_eq!(prev_ref.value(), 0x1000);
            expect_eq!(slot_ref.reference().value(), 0x2000);

            let mut prev = slot_ref.swap_content(VmPageOrMarker::marker());
            expect_true!(prev.is_reference());
            expect_true!(slot_ref.is_marker());
            let _ = prev.release_reference();
        }

        let mut dirty_zero =
            VmPageOrMarker::zero_interval(SentinelType::Start, ZeroRangeDirtyState::Dirty);
        {
            let mut dirty_ref = VmPageOrMarkerRef::new(&mut dirty_zero);
            dirty_ref.set_zero_interval_awaiting_clean_length(8192);
            expect_eq!(dirty_ref.zero_interval_awaiting_clean_length(), 8192);
        }
    }

    /// Tests VmPageListNode layout size, offset/index math, and slot lookups.
    #[test]
    fn test_node_basic() {
        let mut node = VmPageListNode::new();
        expect_eq!(core::mem::size_of::<VmPageListNode>(), 64);
        expect_eq!(VmPageListNode::PAGE_FAN_OUT, 16);
        expect_eq!(VmPageListNode::NODE_SPAN_BYTES, 65536);
        expect_true!(node.is_empty());
        expect_true!(node.has_no_page_or_ref());
        expect_true!(node.has_no_page_ref_or_marker());
        expect_true!(node.has_no_interval_sentinel());

        expect_eq!(VmPageListNode::node_offset(4096), 0);
        expect_eq!(VmPageListNode::node_offset(65536), 65536);
        expect_eq!(VmPageListNode::node_index(4096), 1);
        expect_eq!(VmPageListNode::end_offset(0), 65536);

        *node.lookup_mut(3) = VmPageOrMarker::marker();
        expect_false!(node.is_empty());
        expect_true!(node.has_no_page_or_ref());
        expect_false!(node.has_no_page_ref_or_marker());
        expect_true!(node.has_no_interval_sentinel());
        expect_true!(node.lookup(3).is_marker());
        *node.lookup_mut(3) = VmPageOrMarker::empty();
        expect_true!(node.is_empty());
    }

    /// Tests in-node interval queries.
    #[test]
    fn test_node_interval_queries() {
        let mut node = VmPageListNode::new();
        *node.lookup_mut(3) =
            VmPageOrMarker::zero_interval(SentinelType::End, ZeroRangeDirtyState::Untracked);
        expect_true!(node.node_starts_in_interval().is_some());
        // slot 1 (before end)
        expect_true!(node.is_offset_in_interval(0, 4096).is_some());
        // slot 4 (after end)
        expect_true!(node.is_offset_in_interval(0, 16384).is_none());

        *node.lookup_mut(3) = VmPageOrMarker::empty();
    }

    /// Tests in-node range iteration and merge_range_onto helper.
    #[test]
    fn test_node_range_helpers() {
        let mut node1 = VmPageListNode::new();
        let mut node2 = VmPageListNode::new();
        *node1.lookup_mut(1) = VmPageOrMarker::marker();

        let mut visited_offset = 0;
        let mut count = 0;
        let _ = node1.for_every_page(0, |_slot, offset| {
            visited_offset = offset;
            count += 1;
            Status::NEXT
        });
        expect_eq!(count, 1);
        expect_eq!(visited_offset, 4096);

        let mut ref_count = 0;
        let mut ref_offset = 0;
        let mut is_marker = false;
        let mut share_count = 0;
        let _ = node1.for_every_page_ref(0, |mut slot_ref, offset| {
            ref_offset = offset;
            is_marker = slot_ref.is_marker();
            slot_ref.increment_marker_share_count();
            share_count = slot_ref.marker_share_count();
            ref_count += 1;
            Status::NEXT
        });
        expect_eq!(ref_count, 1);
        expect_eq!(ref_offset, 4096);
        expect_true!(is_marker);
        expect_eq!(share_count, 1);
        expect_eq!(node1.lookup(1).marker_share_count(), 1);

        node1.merge_range_onto(
            0,
            65536,
            |src, dst, _off| *dst = src.swap(VmPageOrMarker::empty()),
            &mut node2,
            0,
            65536,
            65536,
        );
        expect_true!(node1.is_empty());
        expect_true!(node2.lookup(1).is_marker());
        *node2.lookup_mut(1) = VmPageOrMarker::empty();
    }
}
