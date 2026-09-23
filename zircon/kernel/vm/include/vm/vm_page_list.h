// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_PAGE_LIST_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_PAGE_LIST_H_

#include <align.h>
#include <bits.h>
#include <lib/btree.h>
#include <lib/fit/function.h>
#include <lib/page/size.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/canary.h>
#include <fbl/intrusive_wavl_tree.h>
#include <fbl/macros.h>
#include <ktl/algorithm.h>
#include <ktl/unique_ptr.h>
#include <vm/page.h>
#include <vm/pmm.h>
#include <vm/vm_constants.h>

class VmPageList;
class VmPageSpliceList;
class VMPLCursor;

// RAII helper for representing content in a page list node. This supports being in one of five
// states
//  * Empty       - Contains nothing.
//  * Page p      - Contains a vm_page 'p'. This 'p' is considered owned by this wrapper and
//                  `ReleasePage` must be called to give up ownership.
//  * Reference r - Contains a reference 'r' to some content. This 'r' is considered owned by this
//                  wrapper and `ReleaseReference` must be called to give up ownership.
//  * Marker      - Indicates that whilst not a page, it is also not empty. Markers can be used to
//                  separate the distinction between "there's no page because we've deduped to the
//                  zero page" (a `Marker` is inserted) and "there's no page because our parent
//                  contains the content" (which is represented as `Empty`).
//  * Interval    - Indicates that this page is part of a sparse page interval. An interval will
//                  have a Start sentinel, and an End sentinel, and all offsets that lie between the
//                  two will be empty. If the interval spans a single page, it will be represented
//                  as a Slot sentinel, which is conceptually the same as both a Start and an End
//                  sentinel.
//  * ParentContent - Indicates that there might be content for this slot, but the page list in the
//                    parent must be checked for it. The different between `Empty`, which can also
//                    indicate that the parent must be searched, and `ParentContent` is up to the
//                    specific VMO.
//
// There are certain invariants that the page list tries to maintain at all times. It might not
// always be possible to enforce these as the checks involved might be expensive, however it is
// important that any code that manipulates the page list abide by them, primarily to keep the
// memory occupied by the page list nodes in check.
// 1. Page list nodes cannot be completely empty i.e. they must contain at least one non-empty slot.
// 2. Any intervals in the page list should span a maximal range. In other words, there should not
// be consecutive intervals in the page list which it would have been possible to represent with a
// single interval instead.
class VmPageOrMarker {
 public:
  // A PageType that otherwise holds a null pointer is considered to be Empty.
  VmPageOrMarker() : raw_(kPageType) {}
  ~VmPageOrMarker() { DEBUG_ASSERT(!IsPageOrRef()); }
  VmPageOrMarker(VmPageOrMarker&& other) noexcept : raw_(other.Release()) {}
  VmPageOrMarker(const VmPageOrMarker&) = delete;
  VmPageOrMarker& operator=(const VmPageOrMarker&) = delete;

  // Minimal wrapper around a uint32_t to provide stronger typing in code to prevent accidental
  // mixing of references and other values.
  // Provides a way to query the required alignment of the references and does debug enforcement of
  // this.
  class ReferenceValue {
   public:
    // kAlignBits represents the number of low bits in a reference that must be zero so they can be
    // used for internal metadata. This is declared here for convenience, and is asserted to be in
    // sync with the private VmPageOrMarker::kTypeBits.
    static constexpr int kAlignBits = 3;

    explicit constexpr ReferenceValue(uint32_t raw) : value_(raw) {
      DEBUG_ASSERT((value_ & BIT_MASK32(kAlignBits)) == 0);
    }

    uint32_t value() const { return value_; }

   private:
    uint32_t value_;
  };

  // Returns a reference to the underlying vm_page*. Is only valid to call if `IsPage` is true.
  vm_page* Page() const {
    DEBUG_ASSERT(IsPage());
    // Do not need to mask any bits out of raw_, since Page has 0's for the type anyway.
    static_assert(kPageType == 0);
    return Pmm::Node().IndexToPage(raw_);
  }

  // Returns the paddr_t of the underlying vm_page*. Is only valid to call if `IsPage` is true. Can
  // be more efficient than performing |Page()->paddr()| as it saves a memory de-reference.
  paddr_t PageAsPaddr() const {
    DEBUG_ASSERT(IsPage());
    // Do not need to mask any bits out of raw_, since Page has 0's for the type anyway.
    static_assert(kPageType == 0);
    return Pmm::Node().IndexToPaddr(raw_);
  }

  ReferenceValue Reference() const {
    DEBUG_ASSERT(IsReference());
    return ReferenceValue(raw_ & ~BIT_MASK32(ReferenceValue::kAlignBits));
  }

  // If this is a page, moves the underlying vm_page* out and returns it. After this IsPage will
  // be false and IsEmpty will be true.
  [[nodiscard]] vm_page* ReleasePage() {
    DEBUG_ASSERT(IsPage());
    // Do not need to mask any bits out of the Release since Page has 0's for the type
    // anyway.
    static_assert(kPageType == 0);
    return Pmm::Node().IndexToPage(Release());
  }

  [[nodiscard]] ReferenceValue ReleaseReference() {
    DEBUG_ASSERT(IsReference());
    return ReferenceValue(Release() & ~BIT_MASK32(ReferenceValue::kAlignBits));
  }

  // Changes the content from a reference to a page and returns the original reference.
  [[nodiscard]] VmPageOrMarker::ReferenceValue SwapReferenceForPage(vm_page_t* p) {
    DEBUG_ASSERT(p);

    VmPageOrMarker::ReferenceValue ref = ReleaseReference();
    *this = VmPageOrMarker::Page(p);

    return ref;
  }

  // Changes the content from a page to a reference and returns the original page.
  [[nodiscard]] vm_page_t* SwapPageForReference(VmPageOrMarker::ReferenceValue ref) {
    vm_page_t* page = ReleasePage();
    *this = VmPageOrMarker::Reference(ref);

    return page;
  }

  // Changes the content from one reference to a different one and returns the original reference.
  [[nodiscard]] VmPageOrMarker::ReferenceValue SwapReferenceForReference(
      VmPageOrMarker::ReferenceValue ref) {
    const VmPageOrMarker::ReferenceValue old = ReleaseReference();
    *this = VmPageOrMarker::Reference(ref);

    return old;
  }

  uint32_t GetMarkerShareCount() const {
    DEBUG_ASSERT(IsMarker());
    return raw_ >> kMarkerShareCountShift;
  }

  void SetMarkerShareCount(uint32_t share_count) {
    DEBUG_ASSERT(IsMarker());
    raw_ = kZeroMarkerType | (share_count << (kMarkerShareCountShift));
  }

  void IncrementMarkerShareCount() {
    DEBUG_ASSERT(IsMarker());
    raw_ += (1 << (kMarkerShareCountShift));
  }

  void DecrementMarkerShareCount() {
    DEBUG_ASSERT(IsMarker());
    // It is invalid to decrement marker share count from zero.
    DEBUG_ASSERT(GetMarkerShareCount() > 0);

    raw_ -= (1 << (kMarkerShareCountShift));
  }

  [[nodiscard]] VmPageOrMarker Swap(VmPageOrMarker&& other) {
    uint32_t ret = raw_;
    raw_ = other.Release();
    return VmPageOrMarker(ret);
  }

  bool IsPage() const { return !IsEmpty() && (GetType() == kPageType); }
  bool IsMarker() const { return GetType() == kZeroMarkerType; }
  bool IsEmpty() const {
    // A PageType that otherwise holds a null pointer is considered to be Empty.
    return raw_ == kPageType;
  }
  bool IsReference() const { return GetType() == kReferenceType; }
  bool IsPageOrRef() const { return IsPage() || IsReference(); }
  bool IsInterval() const { return GetType() == kIntervalType; }
  bool IsParentContent() const { return GetType() == kParentContentType; }

  VmPageOrMarker& operator=(VmPageOrMarker&& other) noexcept {
    // Forbid overriding content, as that would leak it.
    DEBUG_ASSERT(!IsPageOrRef());
    raw_ = other.Release();
    return *this;
  }

  bool operator==(const VmPageOrMarker& other) const { return raw_ == other.raw_; }

  bool operator!=(const VmPageOrMarker& other) const { return raw_ != other.raw_; }

  // A PageType that otherwise holds a null pointer is considered to be Empty.
  static VmPageOrMarker Empty() { return VmPageOrMarker{kPageType}; }
  static VmPageOrMarker Marker() { return VmPageOrMarker{kZeroMarkerType}; }
  static VmPageOrMarker Marker(uint32_t share_count) {
    return VmPageOrMarker{kZeroMarkerType | (share_count << (kMarkerShareCountShift))};
  }
  static VmPageOrMarker ParentContent() { return VmPageOrMarker{kParentContentType}; }

  [[nodiscard]] static VmPageOrMarker Page(vm_page* p) {
    // Ensure the pmm page-to-index has enough zero bits.
    static_assert(kTypeBits <= PmmNode::kIndexZeroBits);
    static_assert(kPmmNodeIndexZeroBits == PmmNode::kIndexZeroBits);
    // A null page is incorrect for two reasons
    // 1. It's a violation of the API of this method
    // 2. A null page cannot be represented internally as this is used to represent Empty
    DEBUG_ASSERT(p);
    const uint32_t raw = Pmm::Node().PageToIndex(p);
    // Getting zero in |raw| means that |p| lives on the stack or the heap. This is not supported.
    DEBUG_ASSERT(raw != 0u);
    // A pointer should be aligned by definition, and hence the low bits should always be zero, but
    // assert this anyway just in case kTypeBits is increased or someone passed an invalid pointer.
    DEBUG_ASSERT((raw & BIT_MASK32(kTypeBits)) == 0);
    return VmPageOrMarker{raw | kPageType};
  }

  [[nodiscard]] static VmPageOrMarker Reference(ReferenceValue ref) {
    return VmPageOrMarker(ref.value() | kReferenceType);
  }

  // Interval type full bit allocation, from LSB to MSB:
  // Type: 2b = 11 | SentinelType: 2b | IntervalType: 2b | DirtyState: 2b | Length: 24b
  // note: Length is only valid when the DirtyState is Dirty.

  // The types of sparse page interval types that are supported.
  enum class IntervalType : uint32_t {
    // Represents a range of zero pages.
    Zero = 0,
    NumTypes,
  };

  // Sentinel types that are used to represent a sparse page interval.
  enum class SentinelType : uint32_t {
    // Represents a single page interval.
    Slot = 0,
    // The first page of a multi-page interval.
    Start,
    // The last page of a multi-page interval.
    End,
    NumSentinels,
  };

  // The remaining bits of an interval type store any information specific to the type of interval
  // being tracked. The ZeroRange class is defined here to group together the encoding of these bits
  // specific to IntervalType::Zero.
  class ZeroRange {
   public:
    // This is the same as kIntervalBits. Equality is asserted later where kIntervalBits is defined.
    static constexpr int kAlignBits = 7;

    explicit constexpr ZeroRange(uint32_t val) : value_(val) {
      DEBUG_ASSERT((value_ & BIT_MASK32(kAlignBits)) == 0);
    }
    // The various dirty states that a zero interval can be in. Refer to VmCowPages::DirtyState for
    // an explanation of the states. Note that an AwaitingClean state is not encoded in the interval
    // state bits. This information is instead stored using the AwaitingCleanLength for convenience,
    // where a non-zero length indicates that the interval is AwaitingClean. Doing this affords
    // more convenient splitting and merging of intervals.
    enum class DirtyState : uint32_t {
      Untracked = 0,
      Clean,
      Dirty,
      NumStates,
    };

    ZeroRange(uint32_t val, DirtyState state) : value_(val) {
      DEBUG_ASSERT((value_ & BIT_MASK32(kAlignBits)) == 0);
      DEBUG_ASSERT(GetDirtyState() == DirtyState::Untracked);
      SetDirtyState(state);
    }
    uint32_t value() const { return value_; }

    // For zero range tracking, we also need to track dirty state information, and if the interval
    // is AwaitingClean, the length that is AwaitingClean.
    static constexpr uint64_t kDirtyStateBits = VM_PAGE_OBJECT_DIRTY_STATE_BITS;
    static_assert(static_cast<uint32_t>(DirtyState::NumStates) <= (1 << kDirtyStateBits));
    static constexpr int kDirtyStateShift = kAlignBits;
    DirtyState GetDirtyState() const {
      return static_cast<DirtyState>((value_ & (BIT_MASK32(kDirtyStateBits) << kDirtyStateShift)) >>
                                     kDirtyStateShift);
    }
    void SetDirtyState(DirtyState state) {
      // Only allow dirty and untracked zero ranges for now.
      DEBUG_ASSERT(state == DirtyState::Dirty || state == DirtyState::Untracked);
      // Clear the old state.
      value_ &= ~(BIT_MASK32(kDirtyStateBits) << kDirtyStateShift);
      // Set the new state.
      value_ |= static_cast<uint32_t>(state) << kDirtyStateShift;
    }

    static constexpr uint64_t kAwaitingCleanLengthShift = kAlignBits + kDirtyStateBits;
    // Assert that we are not overlapping with the dirty state bits.
    static_assert(kAwaitingCleanLengthShift >= kDirtyStateShift + kDirtyStateBits);
    static_assert(kAwaitingCleanLengthShift <= kPageShift);

    // The AwaitingCleanLength will always be a page-aligned length, so we can mask out the low
    // kPageShift bits and store only the upper bits.
    void SetAwaitingCleanLength(uint64_t len) {
      DEBUG_ASSERT(len == 0 || GetDirtyState() == DirtyState::Dirty);
      DEBUG_ASSERT(IS_ROUNDED(len, kPageSize));
      len = (len >> kPageShift) << kAwaitingCleanLengthShift;
      // Clear the old value.
      value_ &= BIT_MASK32(kAwaitingCleanLengthShift);
      // Set the new value.
      value_ |= static_cast<uint32_t>(len);
    }
    uint64_t GetAwaitingCleanLength() const {
      uint64_t len = value_ & ~BIT_MASK32(kAwaitingCleanLengthShift);
      return (len >> kAwaitingCleanLengthShift) << kPageShift;
    }

   private:
    uint32_t value_;
  };
  using IntervalDirtyState = ZeroRange::DirtyState;

  // Getters and setters for the interval type.
  bool IsIntervalStart() const {
    return IsInterval() && GetIntervalSentinel() == SentinelType::Start;
  }
  bool IsIntervalEnd() const { return IsInterval() && GetIntervalSentinel() == SentinelType::End; }
  bool IsIntervalSlot() const {
    return IsInterval() && GetIntervalSentinel() == SentinelType::Slot;
  }
  bool IsIntervalZero() const { return IsInterval() && GetIntervalType() == IntervalType::Zero; }

  // Getters and setter for the zero interval type.
  bool IsZeroIntervalClean() const {
    DEBUG_ASSERT(IsIntervalZero());
    return ZeroRange(raw_ & ~BIT_MASK32(kIntervalBits)).GetDirtyState() ==
           ZeroRange::DirtyState::Clean;
  }
  bool IsZeroIntervalDirty() const {
    DEBUG_ASSERT(IsIntervalZero());
    return ZeroRange(raw_ & ~BIT_MASK32(kIntervalBits)).GetDirtyState() ==
           ZeroRange::DirtyState::Dirty;
  }
  bool IsZeroIntervalUntracked() const {
    DEBUG_ASSERT(IsIntervalZero());
    return ZeroRange(raw_ & ~BIT_MASK32(kIntervalBits)).GetDirtyState() ==
           ZeroRange::DirtyState::Untracked;
  }
  ZeroRange::DirtyState GetZeroIntervalDirtyState() const {
    DEBUG_ASSERT(IsIntervalZero());
    return ZeroRange(raw_ & ~BIT_MASK32(kIntervalBits)).GetDirtyState();
  }
  void SetZeroIntervalAwaitingCleanLength(uint64_t len) {
    DEBUG_ASSERT(IsIntervalZero());
    DEBUG_ASSERT(IsIntervalStart() || IsIntervalSlot());
    auto interval = ZeroRange(raw_ & ~BIT_MASK32(kIntervalBits));
    interval.SetAwaitingCleanLength(len);
    raw_ = (raw_ & BIT_MASK32(kIntervalBits)) | interval.value();
  }
  uint64_t GetZeroIntervalAwaitingCleanLength() const {
    DEBUG_ASSERT(IsIntervalZero());
    DEBUG_ASSERT(IsIntervalStart() || IsIntervalSlot());
    return ZeroRange(raw_ & ~BIT_MASK32(kIntervalBits)).GetAwaitingCleanLength();
  }

 private:
  explicit VmPageOrMarker(uint32_t raw) : raw_(raw) {}

  // The low 3 bits of raw_ are reserved to represent the type, any other data has to fit into
  // the remaining high bits. Note that there is no explicit Empty type, rather a PageType with a
  // zero pointer is used to represent Empty.
  static constexpr uint32_t kTypeBits = 3;
  static constexpr uint32_t kPageType = 0b000;
  static constexpr uint32_t kZeroMarkerType = 0b001;
  static constexpr uint32_t kReferenceType = 0b010;
  static constexpr uint32_t kIntervalType = 0b011;
  static constexpr uint32_t kParentContentType = 0b100;

  static constexpr uint32_t kMarkerShareCountShift = kTypeBits;

  // Ensure the reference values have alignment such the type bits can be set without overlapping
  // actual ref being stored. Unlike the page type, which does not allow the 0 value to be stored, a
  // ref value of 0 is valid and may be stored.
  static_assert(ReferenceValue::kAlignBits == kTypeBits);

  // In addition to storing the type for an interval, we also need to track the type of interval
  // sentinel: the start, the end, or a single slot marker.
  static constexpr int kIntervalSentinelBits = 2;
  static_assert(static_cast<int>(SentinelType::NumSentinels) <= (1 << kIntervalSentinelBits));
  static constexpr int kIntervalSentinelShift = kTypeBits;
  SentinelType GetIntervalSentinel() const {
    return static_cast<SentinelType>(
        (raw_ & (BIT_MASK(kIntervalSentinelBits) << kIntervalSentinelShift)) >>
        kIntervalSentinelShift);
  }
  void SetIntervalSentinel(SentinelType sentinel) {
    // Clear the old sentinel type.
    raw_ &= ~(BIT_MASK32(kIntervalSentinelBits) << kIntervalSentinelShift);
    // Set the new sentinel type.
    raw_ |= static_cast<uint32_t>(sentinel) << kIntervalSentinelShift;
  }
  // Next we also need to store the type of interval being represented; reserve a couple of bits for
  // this. Currently we only support one type of interval: a range of zero pages, but reserving 2
  // bits allows for more types in the future.
  static constexpr uint64_t kIntervalTypeBits = 2;
  static_assert(static_cast<uint32_t>(IntervalType::NumTypes) <= (1 << kIntervalTypeBits));
  static constexpr uint64_t kIntervalTypeShift = kIntervalSentinelShift + kIntervalSentinelBits;
  IntervalType GetIntervalType() const {
    return static_cast<IntervalType>((raw_ & (BIT_MASK(kIntervalTypeBits) << kIntervalTypeShift)) >>
                                     kIntervalTypeShift);
  }
  static constexpr uint64_t kIntervalBits = kTypeBits + kIntervalSentinelBits + kIntervalTypeBits;
  static_assert(ZeroRange::kAlignBits == kIntervalBits);

  // Only support creation of zero interval type for now.
  // Private and only friended with VmPageList so that an external caller cannot arbitrarily create
  // interval sentinels.
  [[nodiscard]] static VmPageOrMarker ZeroInterval(SentinelType sentinel,
                                                   IntervalDirtyState state) {
    uint32_t sentinel_bits = static_cast<uint32_t>(sentinel) << kIntervalSentinelShift;
    uint32_t type_bits = static_cast<uint32_t>(IntervalType::Zero) << kIntervalTypeShift;
    return VmPageOrMarker(ZeroRange(0, state).value() | type_bits | sentinel_bits | kIntervalType);
  }

  // Change the interval sentinel type for an existing interval, while preserving the rest of the
  // original state. Only valid to call on an existing interval type. The only permissible
  // transitions are from Slot to Start/End and vice versa, as these are the only valid transitions
  // when extending or clipping intervals.
  // Private and only friended with VmPageList so that an external caller cannot arbitrarily
  // manipulate interval sentinels.
  void ChangeIntervalSentinel(SentinelType new_sentinel) {
#if ZX_DEBUG_ASSERT_IMPLEMENTED
    DEBUG_ASSERT(IsInterval());
    auto old_sentinel = GetIntervalSentinel();
    DEBUG_ASSERT(old_sentinel != new_sentinel);
    if (old_sentinel == SentinelType::Start || old_sentinel == SentinelType::End) {
      DEBUG_ASSERT(new_sentinel == SentinelType::Slot);
    } else {
      DEBUG_ASSERT(old_sentinel == SentinelType::Slot);
      DEBUG_ASSERT(new_sentinel == SentinelType::Start || new_sentinel == SentinelType::End);
    }
#endif
    SetIntervalSentinel(new_sentinel);
  }

  uint32_t GetType() const { return raw_ & BIT_MASK(kTypeBits); }

  uint32_t Release() {
    const uint32_t p = raw_;
    raw_ = 0;
    return p;
  }

  uint32_t raw_;

  static_assert(kTypeBits == kVmPageListTypeBits);
  static_assert(kPageType == kVmPageListPageType);
  static_assert(kReferenceType == kVmPageListReferenceType);
  static_assert(kZeroMarkerType == kVmPageListZeroMarkerType);
  static_assert(kIntervalType == kVmPageListIntervalType);
  static_assert(kParentContentType == kVmPageListParentContentType);

  friend VmPageList;
};

// Limited reference to a VmPageOrMarker. This reference provides unrestricted const access to the
// underlying VmPageOrMarker, but as it holds a non-const VmPageOrMarker* it has the ability to
// modify the underlying entry. However, the interface for modification is very limited.
//
// This allows for the majority of VmPageList iterations that are not intended to allow for clearing
// entries to the Empty state to allow limited mutation (such as between different content states),
// without being completely mutable.
class VmPageOrMarkerRef {
 public:
  VmPageOrMarkerRef() = default;
  explicit VmPageOrMarkerRef(VmPageOrMarker* page_or_marker) : page_or_marker_(page_or_marker) {}
  ~VmPageOrMarkerRef() = default;

  const VmPageOrMarker& operator*() const {
    DEBUG_ASSERT(page_or_marker_);
    return *page_or_marker_;
  }

  const VmPageOrMarker* operator->() const {
    DEBUG_ASSERT(page_or_marker_);
    return page_or_marker_;
  }

  explicit operator bool() const { return !!page_or_marker_; }

  // Changing the kind of content is an allowed mutation and this takes ownership of the provided
  // page and returns ownership of the previous reference.
  [[nodiscard]] VmPageOrMarker::ReferenceValue SwapReferenceForPage(vm_page_t* p) {
    DEBUG_ASSERT(page_or_marker_);
    return page_or_marker_->SwapReferenceForPage(p);
  }
  // Similar to SwapReferenceForPage, but takes ownership of the ref and returns ownership of the
  // previous page.
  [[nodiscard]] vm_page_t* SwapPageForReference(VmPageOrMarker::ReferenceValue ref) {
    DEBUG_ASSERT(page_or_marker_);
    return page_or_marker_->SwapPageForReference(ref);
  }
  // Similar to SwapReferenceForPage, but changes one reference for another.
  [[nodiscard]] VmPageOrMarker::ReferenceValue SwapReferenceForReference(
      VmPageOrMarker::ReferenceValue ref) {
    DEBUG_ASSERT(page_or_marker_);
    return page_or_marker_->SwapReferenceForReference(ref);
  }

  // Replaces the contents of this VmPageOrMarker with some non-empty contents, and returns what was
  // previously present. The previous content is allowed to be empty, but the provided content must
  // be non-empty.
  [[nodiscard]] VmPageOrMarker SwapContent(VmPageOrMarker&& content) {
    DEBUG_ASSERT(!content.IsEmpty());
    DEBUG_ASSERT(page_or_marker_);
    return page_or_marker_->Swap(ktl::move(content));
  }

  // Forward dirty state updates as an allowed mutation.
  void SetZeroIntervalAwaitingCleanLength(uint64_t len) {
    DEBUG_ASSERT(page_or_marker_);
    page_or_marker_->SetZeroIntervalAwaitingCleanLength(len);
  }

  uint32_t GetMarkerShareCount() {
    DEBUG_ASSERT(page_or_marker_);
    return page_or_marker_->GetMarkerShareCount();
  }

  void IncrementMarkerShareCount() {
    DEBUG_ASSERT(page_or_marker_);
    page_or_marker_->IncrementMarkerShareCount();
  }

  void DecrementMarkerShareCount() {
    DEBUG_ASSERT(page_or_marker_);
    page_or_marker_->DecrementMarkerShareCount();
  }

 private:
  VmPageOrMarker* page_or_marker_ = nullptr;
};

class VmPageListNode;
// Deletes VmPageListNode's allocated from VmPageListNode::Create. Must not be used with nodes
// created any other way.
struct VmPageListNodeDeleter {
  void operator()(VmPageListNode*);
};

// VmPlnOwner wraps VmPageListNode in a unique_ptr that has single ownership and deletes it using
// VmPageListNodeDeleter when it goes out of scope.
using VmPlnOwner = ktl::unique_ptr<VmPageListNode, VmPageListNodeDeleter>;
class VmPageListNode final {
 public:
  VmPageListNode() = default;
  ~VmPageListNode();

  DISALLOW_COPY_ASSIGN_AND_MOVE(VmPageListNode);

  static constexpr size_t kPageFanOut = kVmPageListFanOut;

  // Creates a new VmPageListNode. Must be deleted using the specified deleter.
  static VmPlnOwner Create();

  static uint64_t end_offset(uint64_t base_offset) {
    DEBUG_ASSERT(NodeOffset(base_offset) == base_offset);
    const uint64_t end = base_offset + (kPageFanOut * kPageSize);
    // By construction the node cannot overflow, but the compiler does not know this. By explicitly
    // telling it some checks can be avoided as the compiler does not have to consider the case
    // where end wrapped.
    if (end <= base_offset) {
      __builtin_unreachable();
    }
    return end;
  }

  // for every page or marker in the node call the passed in function.
  template <typename PTR_TYPE, typename F>
  zx_status_t ForEveryPage(uint64_t base, F func) {
    return ForEveryPageInRange<PTR_TYPE>(this, base, func, base, end_offset(base));
  }

  // for every page or marker in the node call the passed in function.
  template <typename PTR_TYPE, typename F>
  zx_status_t ForEveryPage(uint64_t base, F func) const {
    return ForEveryPageInRange<PTR_TYPE>(this, base, func, base, end_offset(base));
  }

  // for every page or marker in the node in the range call the passed in function. The range is
  // assumed to be within the nodes object range.
  template <typename PTR_TYPE, typename F>
  zx_status_t ForEveryPageInRange(uint64_t base, F func, uint64_t start_offset,
                                  uint64_t end_offset) {
    return ForEveryPageInRange<PTR_TYPE>(this, base, func, start_offset, end_offset);
  }

  // for every page or marker in the node in the range call the passed in function. The range is
  // assumed to be within the nodes object range.
  template <typename PTR_TYPE, typename F>
  zx_status_t ForEveryPageInRange(uint64_t base, F func, uint64_t start_offset,
                                  uint64_t end_offset) const {
    return ForEveryPageInRange<PTR_TYPE>(this, base, func, start_offset, end_offset);
  }

  // Checks if the given offset is part of an interval involving this node. This method cannot find
  // the full interval, since that may require looking at an additional node, but can determine if
  // in an interval or not. Returns any interval sentinel found, otherwise a nullptr.
  const VmPageOrMarker* IsOffsetInInterval(uint64_t obj_offset, uint64_t off) const {
    DEBUG_ASSERT(off >= obj_offset);
    DEBUG_ASSERT(off < end_offset(obj_offset));
    const size_t index = (off - obj_offset) / kPageSize;
    // If the target slot is any kind of interval (start, end, individual slot), then we are in an
    // interval.
    if (!pages_[index].IsEmpty()) {
      return (pages_[index].IsInterval()) ? &pages_[index] : nullptr;
    }
    // Check if there is an interval end to the right, which would cause this to be in an interval.
    // Finding anything else indicates we cannot be in an interval.
    for (size_t i = index + 1; i < kPageFanOut; i++) {
      if (!pages_[i].IsEmpty()) {
        return pages_[i].IsIntervalEnd() ? &pages_[i] : nullptr;
      }
    }
    // Nothing to our right, so check for an interval start to our left.
    for (size_t i = index; i > 0; i--) {
      if (!pages_[i - 1].IsEmpty()) {
        return pages_[i - 1].IsIntervalStart() ? &pages_[i - 1] : nullptr;
      }
    }
    panic("Unexpected empty node");
    return nullptr;
  }

  // Check if this node begins in an interval, that is if an interval start was in a preceding node
  // and this nodes contains the end. If the first non-empty slot is an interval end it is returned,
  // otherwise we cannot have started in an interval and a nullptr is returned.
  const VmPageOrMarker* NodeStartsInInterval() const {
    for (size_t i = 0; i < kPageFanOut; i++) {
      if (!pages_[i].IsEmpty()) {
        return pages_[i].IsIntervalEnd() ? &pages_[i] : nullptr;
      }
    }
    panic("Unexpected empty node");
    return nullptr;
  }

  const VmPageOrMarker& Lookup(size_t index) const {
    DEBUG_ASSERT(index < kPageFanOut);
    return pages_[index];
  }

  VmPageOrMarker& Lookup(size_t index) {
    DEBUG_ASSERT(index < kPageFanOut);
    return pages_[index];
  }

  // A node is empty if it contains no pages, page interval sentinels, references, or markers.
  bool IsEmpty() const {
    for (const auto& p : pages_) {
      if (!p.IsEmpty()) {
        return false;
      }
    }
    return true;
  }

  // Returns true if there are no pages or references owned by this node. Meant to check whether the
  // node has any resource that needs to be returned.
  bool HasNoPageOrRef() const {
    for (const auto& p : pages_) {
      if (p.IsPageOrRef()) {
        return false;
      }
    }
    return true;
  }

  // Returns true if there are no pages, references or markers owned by this node. Meant to check
  // whether the node has any resource that needs to be returned.
  bool HasNoPageRefOrMarker() const {
    for (const auto& p : pages_) {
      if (p.IsPageOrRef() || p.IsMarker()) {
        return false;
      }
    }
    return true;
  }

  // Returns true if there are no interval sentinels owned by this node.
  bool HasNoIntervalSentinel() const {
    for (const auto& p : pages_) {
      if (p.IsInterval()) {
        return false;
      }
    }
    return true;
  }

  template <typename F>
  void MergeRangeOnto(uint64_t base, uint64_t other_base, F migrate_fn, VmPageListNode& other,
                      uint64_t start_offset, uint64_t end_offset, uint64_t other_start_offset) {
    DEBUG_ASSERT(other_start_offset >= other_base);
    DEBUG_ASSERT(other_start_offset + (end_offset - start_offset) <=
                 VmPageListNode::end_offset(other_base));
    ForEveryPageInRange<VmPageOrMarker*>(
        base,
        [&](VmPageOrMarker* slot, uint64_t offset) {
          const uint64_t other_offset = offset - start_offset + other_start_offset;
          DEBUG_ASSERT(NodeOffset(other_offset) == other_base);
          migrate_fn(slot, &other.pages_[NodeIndex(other_offset)], other_offset);
          return ZX_ERR_NEXT;
        },
        start_offset, end_offset);
  }

  // Converts the supplied offset into a VmPageListNode base offset.
  static uint64_t NodeOffset(uint64_t offset) {
    return ROUNDDOWN(offset, kPageSize * VmPageListNode::kPageFanOut);
  }

  // Converts the supplied offset into a VmPageListNode index.
  static uint64_t NodeIndex(uint64_t offset) {
    return (offset >> kPageShift) % VmPageListNode::kPageFanOut;
  }

 private:
  template <typename PTR_TYPE, typename S, typename F>
  static zx_status_t ForEveryPageInRange(S self, uint64_t base, F func, uint64_t start_offset,
                                         uint64_t end_offset) {
    // Assert that the requested range is sensible and falls within our nodes actual offset range.
    DEBUG_ASSERT(end_offset >= start_offset);
    DEBUG_ASSERT(start_offset >= base);
    DEBUG_ASSERT(end_offset <= VmPageListNode::end_offset(base));
    const size_t start = (start_offset - base) / kPageSize;
    const size_t end = (end_offset - base) / kPageSize;
    for (size_t i = start; i < end; i++) {
      if (!self->pages_[i].IsEmpty()) {
        zx_status_t status = func(PTR_TYPE{&self->pages_[i]}, base + i * kPageSize);
        if (status != ZX_ERR_NEXT) {
          return status;
        }
      }
    }
    return ZX_ERR_NEXT;
  }

  VmPageOrMarker pages_[kPageFanOut];

  friend VMPLCursor;
};

// Cursor that can be used for iterating over contiguous blocks of entries in a page list. The
// underlying page list must not have any entries removed while using this cursor, as the cursor
// retains iterators into the page list. It is, however, safe to insert new entries.
// The cursor can be used to iterate over empty contiguous slots, however iteration will always
// cease if entries are not contiguous.
class VMPLCursor : private btree::BTree<uint64_t, VmPlnOwner>::iterator {
 public:
  VMPLCursor() : index_(kPageFanOut) {}

  // See VMPLCursor::current.
  VmPageOrMarkerRef current_ref() const {
    if (valid()) {
      auto [node_offset, node] = get();
      return VmPageOrMarkerRef(&node->pages_[index_]);
    }
    return VmPageOrMarkerRef(nullptr);
  }

  // Retrieve the current VmPageOrMarker pointed at by the cursor. This will be a nullptr if the
  // cursor is no longer valid. The slot pointed at may itself be empty.
  // Note that it is up to the caller to know the offset, which it can track by remembering how
  // many |step|s it has done.
  const VmPageOrMarker* current() const {
    if (valid()) {
      auto [node_offset, node] = get();
      return &node->pages_[index_];
    }
    return nullptr;
  }

  // Move the cursor to the next entry. The next entry can then be retrieved by calling |current|,
  // and if there is no next entry then current will return a nullptr.
  void step() {
    if (valid()) {
      index_++;
      if (index_ == kPageFanOut) {
        inc_node();
      }
    }
  }

  // Calls the provided callback of type [](const VmPageOrMarker*)->zx_status_t on every entry as
  // long as they are contiguous. This is equivalent a loop calling |step| and |current|, but can
  // produce more optimal code gen with the internal loop.
  // The callback can return ZX_ERR_NEXT to continue, ZX_ERR_STOP to cease iteration gracefully, or
  // any other status to terminate with that status code.
  template <typename F>
  zx_status_t ForEveryContiguous(F func) {
    while (valid()) {
      auto [node_offset, node] = get();
      while (index_ < kPageFanOut) {
        const VmPageOrMarker* slot = &node->pages_[index_];
        zx_status_t status = func(slot);
        if (status != ZX_ERR_NEXT) {
          return status == ZX_ERR_STOP ? ZX_OK : status;
        }
        index_++;
      }
      if (!inc_node()) {
        return ZX_OK;
      }
    }
    return ZX_OK;
  }

  // Returns the offset of the |current| position of the cursor. This is invalid to call if
  // |current| is returning a nullptr.
  uint64_t offset() const {
    DEBUG_ASSERT(valid());
    auto [node_offset, node] = get();
    return node_offset + (static_cast<uint64_t>(index_) * kPageSize);
  }

 private:
  static constexpr size_t kPageFanOut = VmPageListNode::kPageFanOut;

  VMPLCursor(btree::BTree<uint64_t, VmPlnOwner>::iterator node, uint index)
      : btree::BTree<uint64_t, VmPlnOwner>::iterator(node), index_(index) {}

  // Helper to increment the underlying node_, testing for contiguity.
  bool inc_node() {
    // Should only be incrementing if index is at the end, as otherwise we're not being contiguous.
    DEBUG_ASSERT(index_ == kPageFanOut);
    auto [prev_offset, _] = get();
    (*this)++;
    if (IsValid()) {
      auto [node_offset, node] = get();
      if (node_offset == prev_offset + kPageSize * kPageFanOut) {
        // node is valid and contiguous, reset the index_ to both remove the terminal sentinel, and
        // resume iteration from the beginning.
        index_ = 0;
        // TODO: Once cursor is in use benchmark the impact of validating that the node is not
        // empty.
        return true;
      }
    }
    return false;
  }

  // Helper to check if the node is valid or not by checking index for its sentinel value.
  bool valid() const { return index_ < kPageFanOut; }

  // The index into node_ that is currently being pointed at to be returned by |current|. The
  // sentinel value of kPageFanOut is used to indicate that node_ is no longer valid.
  uint index_;

  friend VmPageList;
};

class VmPageList final {
 public:
  VmPageList();
  ~VmPageList();

  VmPageList& operator=(VmPageList&& other);
  VmPageList(VmPageList&& other);

  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(VmPageList);

  // walk the page tree, calling the passed in function on every tree node.
  template <typename F>
  zx_status_t ForEveryPage(F per_page_func) const {
    return ForEveryPage<const VmPageOrMarker*>(this, per_page_func);
  }

  // similar to ForEveryPage, but the per_page_func gets called with a VmPageOrMarkerRef instead of
  // a const VmPageOrMarker*, allowing for limited mutation.
  template <typename F>
  zx_status_t ForEveryPageMutable(F per_page_func) {
    return ForEveryPage<VmPageOrMarkerRef>(this, per_page_func);
  }

  // walk the page tree, calling the passed in function on every tree node.
  template <typename F>
  zx_status_t ForEveryPageInRange(F per_page_func, uint64_t start_offset,
                                  uint64_t end_offset) const {
    return ForEveryPageInRange<const VmPageOrMarker*>(this, per_page_func, start_offset,
                                                      end_offset);
  }

  // similar to ForEveryPageInRange but uses a valid VMPLCursor as the starting point.
  template <typename F>
  zx_status_t ForEveryPageInCursorRange(F per_page_func, VMPLCursor cursor,
                                        uint64_t end_offset) const {
    const uint64_t start_offset = cursor.offset();
    if (start_offset >= end_offset) {
      return ZX_OK;
    }
    return ForEveryPageInRangeInternal<const VmPageOrMarker*, NodeCheck::Skip>(
        this, per_page_func, *static_cast<btree::BTree<uint64_t, VmPlnOwner>::iterator*>(&cursor),
        start_offset, end_offset);
  }

  // similar to ForEveryPageInRange, but the per_page_func gets called with a VmPageOrMarkerRef
  // instead of a const VmPageOrMarker*, allowing for limited mutation.
  template <typename F>
  zx_status_t ForEveryPageInRangeMutable(F per_page_func, uint64_t start_offset,
                                         uint64_t end_offset) {
    return ForEveryPageInRange<VmPageOrMarkerRef>(this, per_page_func, start_offset, end_offset);
  }

  // walk the page tree, calling |per_page_func| on every page/marker and |per_gap_func| on every
  // gap.
  template <typename PAGE_FUNC, typename GAP_FUNC>
  zx_status_t ForEveryPageAndGapInRange(PAGE_FUNC per_page_func, GAP_FUNC per_gap_func,
                                        uint64_t start_offset, uint64_t end_offset) const {
    return ForEveryPageAndGapInRange<const VmPageOrMarker*>(this, per_page_func, per_gap_func,
                                                            start_offset, end_offset);
  }
  template <typename PAGE_FUNC, typename GAP_FUNC>
  zx_status_t ForEveryPageAndGapInRangeMutable(PAGE_FUNC per_page_func, GAP_FUNC per_gap_func,
                                               uint64_t start_offset, uint64_t end_offset) {
    return ForEveryPageAndGapInRange<VmPageOrMarkerRef>(this, per_page_func, per_gap_func,
                                                        start_offset, end_offset);
  }

  // walk the page tree, calling |per_page_func| on every page/marker/interval that fulfills
  // (returns true) the |compare_func|. Also call |contiguous_run_func| on every contiguous range of
  // such pages/markers/intervals encountered, whose signature is:
  // zx_status_t contiguous_run_func(uint64_t start, uint64_t end, bool is_interval)
  //
  // Intervals are treated as distinct contiguous runs, i.e. they won't be merged into a contiguous
  // run of pages/markers for invocation of |contiguous_run_func|. For intervals,
  // |contiguous_run_func| will be called with |is_interval| set to true; for other page types it
  // will be false. Additionally, the entire interval should fulfill |compare_func| for
  // |contiguous_run_func| to be called on the portion that falls in [start_offset, end_offset).
  template <typename COMPARE_FUNC, typename PAGE_FUNC, typename CONTIGUOUS_RUN_FUNC>
  zx_status_t ForEveryPageAndContiguousRunInRange(COMPARE_FUNC compare_func,
                                                  PAGE_FUNC per_page_func,
                                                  CONTIGUOUS_RUN_FUNC contiguous_run_func,
                                                  uint64_t start_offset,
                                                  uint64_t end_offset) const {
    return ForEveryPageAndContiguousRunInRange<const VmPageOrMarker*>(
        this, compare_func, per_page_func, contiguous_run_func, start_offset, end_offset);
  }

  // Returns true if any pages (actual pages, references, or markers) are in the given range, or if
  // the range forms a part of a sparse page interval.
  bool AnyPagesOrIntervalsInRange(uint64_t start_offset, uint64_t end_offset) const {
    bool found_page = false;
    ForEveryPageInRange(
        [&found_page](const VmPageOrMarker* page, uint64_t offset) {
          found_page = true;
          return ZX_ERR_STOP;
        },
        start_offset, end_offset);
    // It is possible that the range forms a part of an interval even if no nodes in the range have
    // populated slots. We can determine that by checking to see if the start offset in the range
    // falls in an interval (we could technically perform this check for any inclusive offset in the
    // range since the range is entirely unpopulated and hence would only fall in the same interval
    // if applicable).
    return found_page ? true : IsOffsetInInterval(start_offset);
  }

  // Similar to |AnyPagesOrIntervalsInRange| but skips over any ParentContent markers, as these do
  // not represent content owned by this page list, but rather content owned by a parent.
  bool AnyOwnedPagesOrIntervalsInRange(uint64_t start_offset, uint64_t end_offset) const {
    bool found_page = false;
    ForEveryPageInRange(
        [&found_page](const VmPageOrMarker* page, uint64_t offset) {
          if (page->IsParentContent()) {
            return ZX_ERR_NEXT;
          }
          found_page = true;
          return ZX_ERR_STOP;
        },
        start_offset, end_offset);
    // It is possible that the range forms a part of an interval even if no nodes in the range have
    // populated slots. We can determine that by checking to see if the start offset in the range
    // falls in an interval (we could technically perform this check for any inclusive offset in the
    // range since the range is entirely unpopulated and hence would only fall in the same interval
    // if applicable).
    return found_page ? true : IsOffsetInInterval(start_offset);
  }

  // Attempts to return a reference to the VmPageOrMarker at the specified offset. The returned
  // pointer is valid until the VmPageList is destroyed or any of the Remove*/Take/Merge etc
  // functions are called.
  //
  // Lookup may return 'nullptr' if there is no slot allocated for the given offset. If non-null
  // is returned it may still be the case that IsEmpty() on the returned PageOrMarker is true.
  const VmPageOrMarker* Lookup(uint64_t offset) const {
    // lookup the tree node that holds this offset
    if (auto pln = list_.find(NodeOffset(offset)); pln.IsValid()) {
      auto [node_offset, node] = *pln;
      return &node->Lookup(NodeIndex(offset));
    }
    return nullptr;
  }

  // Similar to `Lookup` but returns a VmPageOrMarkerRef that allows for limited mutation of the
  // slot. General mutation requires calling `LookupOrAllocate`.
  VmPageOrMarkerRef LookupMutable(uint64_t offset) {
    // lookup the tree node that holds this offset
    if (auto pln = list_.find(NodeOffset(offset)); pln.IsValid()) {
      auto [node_offset, node] = *pln;
      return VmPageOrMarkerRef(&node->Lookup(NodeIndex(offset)));
    }
    return VmPageOrMarkerRef(nullptr);
  }

  // Similar to `LookupMutable` but returns a VMPLCursor that allows for iterating over any
  // contiguous slots from the provided offset.
  VMPLCursor LookupMutableCursor(uint64_t offset) {
    // lookup the tree node that holds this offset
    NodeList::iterator pln = list_.find(NodeOffset(offset));
    if (!pln.IsValid()) {
      return VMPLCursor();
    }
    return VMPLCursor(pln, static_cast<uint>(NodeIndex(offset)));
  }

  // Similar to `LookupMutableCursor` but does a lower_bound search instead of a find, returning the
  // first slot >= offset, if any exists.
  VMPLCursor LookupNearestMutableCursor(uint64_t offset) {
    // lookup the tree node that holds this offset or a larger one.
    const uint64_t node_offset = NodeOffset(offset);
    if (NodeList::iterator pln = list_.lower_bound(node_offset); pln.IsValid()) {
      auto [found_offset, _] = *pln;
      const uint64_t index = found_offset == node_offset ? NodeIndex(offset) : 0;
      return VMPLCursor(pln, static_cast<uint>(index));
    }
    return VMPLCursor();
  }

  // The interval handling flag to be used by LookupOrAllocate. See comments near LookupOrAllocate.
  enum class IntervalHandling : uint8_t {
    NoIntervals,
    CheckForInterval,
    SplitInterval,
  };
  // Similar to `Lookup` but only returns `nullptr` if a slot cannot be allocated either due to out
  // of memory, due to offset being invalid, or |interval_handling| not allowing for a slot to be
  // safely returned.
  //
  // The returned slot, if not a `nullptr`, may generally be freely manipulated with the exception
  // that if it started !Empty, then it is an error to set it to Empty. In this case the
  // `RemovePage` method must be used.
  //
  // If the returned slot started Empty, as it not made !Empty, then the slot must be returned with
  // ReturnEmptySlot, to ensure no empty nodes are retained.
  //
  // The bool in the ktl::pair returns whether the offset falls inside a sparse interval. And
  // whether a valid VmPageOrMarker* is returned in the ktl::pair depends on the specified
  // |interval_handling|.
  //  - NoIntervals: The page list does not contain any intervals, so there is no special handling
  //  to check for or split intervals. In other words, each slot in the page list can be manipulated
  //  independently.
  //  - CheckForIntervals: The page list can contain intervals, and the bool in the returned
  //  ktl::pair indicates whether the offset fell inside an interval. Note that this only checks for
  //  intervals but does not allow manipulating them, so a valid VmPageOrMarker* will be returned
  //  only if the offset can safely be manipulated independently.
  //  - SplitInterval: The page list can contain intervals and we are allowed to split intervals to
  //  return the required slot. The returned VmPageOrMarker* can be manipulated freely. (See
  //  comments near LookupOrAllocateCheckForInterval for an explanation of how splitting works.)
  ktl::pair<VmPageOrMarker*, bool> LookupOrAllocate(uint64_t offset,
                                                    IntervalHandling interval_handling) {
    switch (interval_handling) {
      case IntervalHandling::NoIntervals:
        // The page list does not expect any intervals. Short circuit any checks for intervals.
        return {LookupOrAllocateInternal(offset), false};
      case IntervalHandling::CheckForInterval:
        // Check for intervals but do not allow splitting them.
        return LookupOrAllocateCheckForInterval(offset, false);
      case IntervalHandling::SplitInterval:
        // Check for intervals and also split them.
        return LookupOrAllocateCheckForInterval(offset, true);
    }
    return {nullptr, false};
  }

  // Helper object for performing repeated LookupOrAllocate operations that are likely to be close
  // to each other. While using this object other (modifying) methods on this VmPageList must not be
  // performed, if they are the |reset| method needs to be used before continuing.
  class BatchInserter {
   public:
    // Construct a BatchInserter for the specified VmPageList. The list must be kept alive for the
    // duration of this object.
    explicit BatchInserter(VmPageList& list) : list_(list.list_) {}
    ~BatchInserter() = default;

    BatchInserter(const BatchInserter&) = delete;
    BatchInserter& operator=(const BatchInserter&) = delete;

    // Similar to VmPageList::LookupOrAllocate but is implicitly NoIntervals. If repeated offsets
    // are 'near' each other (in the same node, or in following nodes) then this will be more
    // efficient than the VmPageList::LookupOrAllocate. However, regardless of the |offset| pattern
    // this will always return correct results.
    VmPageOrMarker* LookupOrAllocate(uint64_t offset);

    // Reset the batch inserter. This makes it safe to use again if other VmPageList operations had
    // been performed.
    void reset() { node_ = {}; }

   private:
    btree::BTree<uint64_t, VmPlnOwner>& list_;
    btree::BTree<uint64_t, VmPlnOwner>::iterator node_;
  };

  // Returns a slot that was empty after LookupOrAllocate, and that the caller did not end up
  // filling.
  // This ensures that if LookupOrAllocate allocated a new underlying list node, then that list node
  // needs to be free'd otherwise it might not get cleaned up for the lifetime of the page list.
  //
  // This is only correct to call on an offset for which LookupOrAllocate had just returned a non
  // null slot, and that slot was Empty and is still Empty.
  void ReturnEmptySlot(uint64_t offset);

  // Removes any item at |offset| from the list and returns it, or VmPageOrMarker::Empty() if none.
  VmPageOrMarker RemoveContent(uint64_t offset);

  // Release and call free_content_fn on every item in the page list. Gives free_content_fn
  // ownership of the content. After calling this method, all slots in the page list are empty.
  template <typename T>
  void RemoveAllContent(T free_content_fn) {
    // per page get a reference to the page pointer inside the page list node
    auto per_page_func = [&free_content_fn](VmPageOrMarker* p, uint64_t offset) {
      free_content_fn(ktl::move(*p));
      *p = VmPageOrMarker::Empty();
      return ZX_ERR_NEXT;
    };

    // walk the tree in order, freeing all the pages on every node
    ForEveryPage<VmPageOrMarker*>(this, per_page_func);

    // empty the tree
    Clear();
  }

  // Clears the tree of any remaining slots, leaving it in the initially allocated state. It is an
  // error, and will trigger a panic, for any of the slots to hold pages or references, as clearing
  // them would otherwise result in a memory leak.
  void Clear() { list_.clear(); }

  // Calls the provided callback for every page or marker in the range [start_offset, end_offset).
  // The callback can modify the VmPageOrMarker and take ownership of any pages, or leave them in
  // place. The difference between this and ForEveryPage is as this allows for modifying the
  // underlying pages any intermediate data structures can be checked and potentially freed if no
  // longer needed.
  template <typename T>
  zx_status_t RemovePages(T per_page_fn, uint64_t start_offset, uint64_t end_offset) {
    return ForEveryPageInRange<VmPageOrMarker*, NodeCheck::CleanupEmpty>(this, per_page_fn,
                                                                         start_offset, end_offset);
  }

  // Similar to RemovePages but also takes a |per_gap_fn| callback to allow for iterating over any
  // gaps encountered as well. This can be used when the intent is to modify the underlying pages
  // and/or gaps, while checking any intermediate data structures to potentially free ones that are
  // no longer needed.
  template <typename P, typename G>
  zx_status_t RemovePagesAndIterateGaps(P per_page_fn, G per_gap_fn, uint64_t start_offset,
                                        uint64_t end_offset) {
    return ForEveryPageAndGapInRange<VmPageOrMarker*, NodeCheck::CleanupEmpty>(
        this, per_page_fn, per_gap_fn, start_offset, end_offset);
  }

  // Returns true if there are no pages, references, markers, or intervals in the page list.
  bool IsEmpty() const { return list_.is_empty(); }

  // Returns true if the page list does not own any pages or references. Meant to check whether the
  // page list has any resource that needs to be returned.
  bool HasNoPageOrRef() const;

  // Similar to `HasNoPageOrRef` but returns false if there is a marker.
  bool HasNoPageRefOrMarker() const;

  // Merges the pages in the specified range in |this| onto the |other| with |offset| in this
  // mapping to the offset of 0 in |other|.
  //
  // For any offset in |this| that is not empty then the given |migrate_fn| is called with a
  // reference to |this| and the corresponding slot in |other| and has the signature of:
  // void migrate_fn(VmPageOrMarker* this_slot, VmPageOrMarker* other_slot, uint64_t other_offset);
  //
  // If this returns |true| then migrate_fn was called on everything in range of |offset| to
  // |end_offset|. If false is returned then merging did not complete due to inability to allocates
  // nodes in |other|. The partial merge is not rolled back and is up to the caller to deal with.
  template <typename F>
  bool MergeRangeOnto(F migrate_fn, VmPageList& other, uint64_t offset, uint64_t end_offset) {
    // Iterate the range in this we are merging.
    for (auto iter = list_.lower_bound(NodeOffset(offset)); iter && (*iter).first < end_offset;) {
      auto [node_obj_offset, node] = *iter;

      DEBUG_ASSERT(node->HasNoIntervalSentinel());
      // Calculate start and end in |node|
      uint64_t node_start_off = ktl::max(node_obj_offset, offset);
      const uint64_t node_end_off =
          ktl::min(VmPageListNode::end_offset(node_obj_offset), end_offset);

      // If offset is not a multiple of kNodeSize then items in this node will need to be split
      // across two different nodes in other.
      while (node_start_off < node_end_off) {
        // Translate the start in |node| to an offset in |other|
        const uint64_t other_start_off = node_start_off - offset;

        // Find the node in |other| that contains the start address.
        const uint64_t other_obj_offset = NodeOffset(other_start_off);
        auto cur_other = other.list_.lower_bound(other_obj_offset);

        // Allocate a new node if none was found.
        if (!cur_other || (*cur_other).first != other_obj_offset) {
          fbl::AllocChecker ac;
          VmPlnOwner pl = VmPageListNode::Create();
          if (!pl) {
            return false;
          }
          cur_other = other.list_.insert(cur_other, other_obj_offset, ktl::move(pl));
          if (!cur_other) {
            return false;
          }
        }
        VmPageListNode* cur_other_node = (*cur_other).second;

        // Cap the range to the end of |cur_other|, which could be less than length of the range in
        // |node|.
        const uint64_t other_end_off =
            ktl::min(VmPageListNode::end_offset(other_obj_offset), node_end_off - offset);

        const uint64_t len = other_end_off - other_start_off;

        node->MergeRangeOnto(node_obj_offset, other_obj_offset, migrate_fn, *cur_other_node,
                             node_start_off, node_start_off + len, other_start_off);

        // If either nothing was transferred, or all existing content was removed, then remove
        // |cur_other|.
        if (cur_other_node->IsEmpty()) {
          other.list_.erase(cur_other);
        }

        node_start_off += len;
      }
      // If all the content was moved out of |node| then remove it. |node| could still have content
      // if |migrate_fn| chose to leave it, or if the range being processed only partially covered
      // |node|.
      if (node->IsEmpty()) {
        iter = list_.erase(iter);
      } else {
        iter++;
      }
    }
    return true;
  }

  uint64_t HeapAllocationBytes() const {
    auto utilization = list_.calculate_utilization_slow();
    return utilization.nodes_in_bytes() + (utilization.stored_values * sizeof(VmPageListNode));
  }

  // Allow the implementation to use a one-past-the-end for VmPageListNode offsets.
  static constexpr uint64_t MAX_SIZE =
      ROUNDDOWN(UINT64_MAX, VmPageListNode::kPageFanOut* kPageSize);

  // Add a sparse zero interval spanning the range [start_offset, end_offset) with the specified
  // dirty_state. The specified range must be previously unpopulated. This will try to merge the new
  // zero interval with existing intervals to the left and/or right, if the dirty_state allows it.
  zx_status_t AddZeroInterval(uint64_t start_offset, uint64_t end_offset,
                              VmPageOrMarker::IntervalDirtyState dirty_state) {
    return AddZeroIntervalInternal(start_offset, end_offset, dirty_state, 0);
  }

  // Populates individual interval slots in the range [start_offset, end_offset) that falls inside a
  // sparse interval. The intent of this function is to allow the caller to prepare the range for
  // overwriting (replacing with pages) by populating the required slots upfront, so that slot
  // lookup does not fail after this call. Essentially simulates interval splits
  // (LookupOrAllocateCheckForInterval) for every offset in the specified range, but does so more
  // efficiently, instead of having to search the tree repeatedly for every single offset.
  zx_status_t PopulateSlotsInInterval(uint64_t start_offset, uint64_t end_offset);

  // Helper to return an unused interval slot so that it can be merged back into the interval it was
  // populated/split from.
  void ReturnIntervalSlot(uint64_t offset);

  // Clips an interval from the start by len, i.e. moves the start from interval_start to
  // interval_start + len. The total length of the interval must be larger than len.
  zx_status_t ClipIntervalStart(uint64_t interval_start, uint64_t len);

  // Clips an interval from the end by len, i.e. moves the end from interval_end to
  // interval_end - len. The total length of the interval must be larger than len.
  zx_status_t ClipIntervalEnd(uint64_t interval_end, uint64_t len);

  // Returns true if the specified offset falls in a sparse zero interval.
  bool IsOffsetInZeroInterval(uint64_t offset) const;

  // Replace an existing page at offset with a zero interval, and return the released page. The
  // caller takes ownership of the released page and is responsible for freeing it.
  vm_page_t* ReplacePageWithZeroInterval(uint64_t offset,
                                         VmPageOrMarker::IntervalDirtyState dirty_state);

  // Overwrite a zero interval either fully or partially with a new zero interval, breaking off the
  // old interval into two if required. old_start_offset and old_end_offset specify the start and
  // end sentinels of the old interval that is being overwritten; either one of these or both can be
  // specified, with the other set to UINT64_MAX. The new zero interval that overwrites the old
  // spans [new_start_offset, new_end_offset] with its state set to new_dirty_state.
  //  - For full overwrites, both old_start_offset and old_end_offset must be provided, and should
  //  be equal to new_start_offset and new_end_offset respectively. At the end of the call, the old
  //  interval will have been completely replaced by the new one.
  //  - For partial overwrites from the start, old_start_offset must be provided and be equal to
  //  new_start_offset, and old_end_offset must be UINT64_MAX. At the end of this call, the start of
  //  the old interval will have been overwritten by the new interval, with the remainder of the old
  //  interval now starting at |new_end_offset + kPageSize|.
  //  - For partial overwrites from the end, old_end_offset must be provided and be equal to
  //  new_end_offset, and old_start_offset must be UINT64_MAX. At the end of this call, the end of
  //  the old interval will have been overwritten by the new interval, with the remainder of the old
  //  interval now ending at |new_start_offset - kPageSize|.
  //  - Partial overwrites in the middle are not allowed. In other words, either old_start_offset
  //  must be the same as new_start_offset, or old_end_offset must be the same as new_end_offset, or
  //  both.
  zx_status_t OverwriteZeroInterval(uint64_t old_start_offset, uint64_t old_end_offset,
                                    uint64_t new_start_offset, uint64_t new_end_offset,
                                    VmPageOrMarker::IntervalDirtyState new_dirty_state);

 private:
  static uint64_t NodeOffset(uint64_t offset) { return VmPageListNode::NodeOffset(offset); }

  static uint64_t NodeIndex(uint64_t offset) { return VmPageListNode::NodeIndex(offset); }

  // Returns true if the specified offset falls in a sparse page interval.
  bool IsOffsetInInterval(uint64_t offset) const;

  // Internal helper used when checking whether the offset falls in an interval.
  // lower_bound is the node that was queried with a lower_bound() lookup on the list using the
  // offset. This node is passed in here so that we can reuse the node the callsite has looked up
  // and avoid an extra lookup. Any interval sentinel found is returned, otherwise a nullptr is
  // returned.
  template <typename T>
  static const VmPageOrMarker* IsOffsetInIntervalHelper(
      uint64_t offset, btree::BTree<uint64_t, VmPlnOwner>::iterator_impl<T> lower_bound) {
    auto [obj_offset, node] = *lower_bound;
    if (offset < obj_offset) {
      return node->NodeStartsInInterval();
    }
    return node->IsOffsetInInterval(obj_offset, offset);
  }

  // Internal helper for AddZeroInterval.
  // |replace_existing_slot| can optionally be set to true if a zero interval spanning a single page
  // is being added, and the slot at that offset is already populated (but Empty) and can be reused.
  zx_status_t AddZeroIntervalInternal(uint64_t start_offset, uint64_t end_offset,
                                      VmPageOrMarker::IntervalDirtyState dirty_state,
                                      uint64_t awaiting_clean_len,
                                      bool replace_existing_slot = false);

  // Internal helper for LookupOrAllocate.
  VmPageOrMarker* LookupOrAllocateInternal(uint64_t offset);

  // Similar to LookupOrAllocateInternal but also checks if offset falls in a sparse page interval,
  // returning true via the bool in ktl::pair if it does, along with the slot. Also splits the
  // interval around offset if split_interval is set to true. This allows the caller to freely
  // manipulate the slot at offset similar to LookupOrAllocate. If offset is found in an interval,
  // but split_interval was false, no VmPageOrMarker* is returned, as it is not safe to manipulate
  // any slot in an interval without also splitting the interval around it.
  //
  // In other words, the return values fall into three categories.
  //  1. {page, false} : offset does not lie in an interval. |slot| is the required slot.
  //  2. {page, true} : offset lies in an interval and split_interval was true. |page| is the
  //  required slot. The interval has been correctly split around the slot, so |page| can be treated
  //  similar to any non-interval type.
  //  3. {nullptr, true} : offset lies in an interval but split_interval was false. No slot is
  //  returned.
  //
  // Splitting the interval would look as follows. If the interval previously was:
  //  [start, end) where start < offset < end.
  // After the split we would have three intervals:
  //  [start, offset) [offset, offset + kPageSize) [offset + kPageSize, end)
  // The middle interval containing offset spans only a single page, i.e. offset is an
  // IntervalSentinel::Slot, which can now be manipulated independently.
  ktl::pair<VmPageOrMarker*, bool> LookupOrAllocateCheckForInterval(uint64_t offset,
                                                                    bool split_interval);

  template <typename PTR_TYPE, typename S, typename F>
  static zx_status_t ForEveryPage(S self, F per_page_func) {
    for (auto [node_offset, node] : self->list_) {
      zx_status_t status = node->template ForEveryPage<PTR_TYPE>(node_offset, per_page_func);
      if (status != ZX_ERR_NEXT) {
        if (status == ZX_ERR_STOP) {
          break;
        }
        return status;
      }
    }
    return ZX_OK;
  }

  // Calls the provided callback for every page in the given range. If the CleanupNodes template
  // argument is true then it is assumed the per_page_func may remove pages and page nodes will be
  // checked to see if they are empty and can be cleaned up.
  enum class NodeCheck : bool {
    Skip = false,
    CleanupEmpty = true,
  };
  template <typename PTR_TYPE, NodeCheck NODE_CHECK, typename S, typename F>
  static zx_status_t ForEveryPageInRangeInternal(S self, F per_page_func, auto cur,
                                                 uint64_t start_offset, uint64_t end_offset) {
    DEBUG_ASSERT(IsPageRounded(start_offset));
    DEBUG_ASSERT(IsPageRounded(end_offset));

    while (cur) {
      auto [cur_offset, node] = *cur;
      if (cur_offset >= end_offset) {
        break;
      }
      uint64_t start = ktl::max(start_offset, cur_offset);
      uint64_t end = ktl::min(VmPageListNode::end_offset(cur_offset), end_offset);
      zx_status_t status =
          node->template ForEveryPageInRange<PTR_TYPE, F>(cur_offset, per_page_func, start, end);
      if constexpr (NODE_CHECK == NodeCheck::CleanupEmpty) {
        if (node->IsEmpty()) {
          cur = self->list_.erase(cur);
        } else {
          cur++;
        }
      } else {
        cur++;
      }
      if (status != ZX_ERR_NEXT) {
        return status;
      }
    }

    return ZX_ERR_NEXT;
  }
  template <typename PTR_TYPE, NodeCheck NODE_CHECK = NodeCheck::Skip, typename S, typename F>
  static zx_status_t ForEveryPageInRange(S self, F per_page_func, uint64_t start_offset,
                                         uint64_t end_offset) {
    // Find the first node (if any) that will contain our starting offset.
    auto cur = self->list_.lower_bound(self->NodeOffset(start_offset));
    zx_status_t status = ForEveryPageInRangeInternal<PTR_TYPE, NODE_CHECK>(
        self, per_page_func, ktl::move(cur), start_offset, end_offset);
    if (status != ZX_ERR_NEXT) {
      if (status == ZX_ERR_STOP) {
        return ZX_OK;
      }
      return status;
    }
    return ZX_OK;
  }

  template <typename PTR_TYPE, NodeCheck NODE_CHECK = NodeCheck::Skip, typename S,
            typename PAGE_FUNC, typename GAP_FUNC>
  static zx_status_t ForEveryPageAndGapInRange(S self, PAGE_FUNC per_page_func,
                                               GAP_FUNC per_gap_func, uint64_t start_offset,
                                               uint64_t end_offset) {
    auto cur = self->list_.lower_bound(self->NodeOffset(start_offset));

    uint64_t expected_next_off = start_offset;
    // Set to true when we encounter an interval start but haven't yet encountered the end.
    bool in_interval = false;
    auto per_page_wrapper_fn = [&](auto p, uint64_t off) {
      // Update our interval tracking first. Should the callbacks later request an early exit then
      // this work is wasted, but doing it first, and unconditionally, lets the compiler perform
      // better common expression elimination with the per_gap_func check next.
      if (p->IsIntervalStart()) {
        // We should not already have been tracking an interval.
        DEBUG_ASSERT(!in_interval);
        // Start and end sentinel interval types should match. Since we only support zero
        // intervals currently, we can simply check for that.
        DEBUG_ASSERT(p->IsIntervalZero());
        in_interval = true;
      } else if (p->IsIntervalEnd()) {
        // If this is not the first populated slot we encountered, we should have been tracking a
        // valid interval.
        DEBUG_ASSERT(in_interval || expected_next_off == start_offset);
        // Start and end sentinel interval types should match. Since we only support zero
        // intervals currently, we can simply check for that.
        DEBUG_ASSERT(p->IsIntervalZero());
        // Reset interval tracking.
        in_interval = false;
      }
      zx_status_t status = ZX_ERR_NEXT;
      // We can move ahead of expected_next_off in the case of an interval too, which represents a
      // run of pages. Make sure this is not an interval before calling the per_gap_func.
      if (expected_next_off != off && !p->IsIntervalEnd()) {
        status = per_gap_func(expected_next_off, off);
      }
      expected_next_off = off + kPageSize;
      if (status == ZX_ERR_NEXT) {
        status = per_page_func(p, off);
      }
      return status;
    };

    zx_status_t status = ForEveryPageInRangeInternal<PTR_TYPE, NODE_CHECK>(
        self, per_page_wrapper_fn, cur, start_offset, end_offset);
    if (status != ZX_ERR_NEXT) {
      if (status == ZX_ERR_STOP) {
        return ZX_OK;
      }
      return status;
    }

    // Handle the last gap after checking that we are not in an interval. Note that simply checking
    // for in_interval is not sufficient, as it is possible to have started the traversal partway
    // into an interval, in which case we would not have seen the interval start and in_interval
    // would be false. So we perform a quick check for in_interval first and if that fails perform
    // the more expensive IsOffsetInInterval() check. The IsOffsetInInterval() call is further gated
    // by whether we encountered any page at all in the traversal above. If we saw at least one
    // page in the traversal, we know that we could not be in an interval without in_interval being
    // true because we would have seen the interval start.
    if (expected_next_off != end_offset) {
      // Traversal ended in an interval if in_interval was true, OR if the traversal did not see any
      // page at all and the start_offset is in an interval (Note that in this latter case all
      // offsets in the range [start_offset, end_offset) would lie in the same interval, so we can
      // just check one of them).
      bool ended_in_interval = in_interval || (expected_next_off == start_offset && cur &&
                                               !!IsOffsetInIntervalHelper(start_offset, cur));
      if (!ended_in_interval) {
        status = per_gap_func(expected_next_off, end_offset);
        if (status != ZX_ERR_NEXT && status != ZX_ERR_STOP) {
          return status;
        }
      }
    }

    return ZX_OK;
  }

  // Internal helpers to return the start (or end) of an interval given the end (or start) along
  // with the corresponding offset.
  ktl::pair<const VmPageOrMarker*, uint64_t> FindIntervalStartForEnd(uint64_t end_offset) const;
  ktl::pair<const VmPageOrMarker*, uint64_t> FindIntervalEndForStart(uint64_t start_offset) const;

  template <typename PTR_TYPE, typename S, typename COMPARE_FUNC, typename PAGE_FUNC,
            typename CONTIGUOUS_RUN_FUNC>
  static zx_status_t ForEveryPageAndContiguousRunInRange(S self, COMPARE_FUNC compare_func,
                                                         PAGE_FUNC per_page_func,
                                                         CONTIGUOUS_RUN_FUNC contiguous_run_func,
                                                         uint64_t start_offset,
                                                         uint64_t end_offset) {
    if (start_offset == end_offset) {
      return ZX_OK;
    }

    // Track contiguous range of pages fulfilling compare_func.
    uint64_t contiguous_run_start = start_offset;
    uint64_t contiguous_run_len = 0;

    // Tracks whether we enter the ForEveryPageAndGap traversal at all.
    bool found_page_or_gap = false;
    // Tracks information if we encounter an interval start, to be used when we encounter the
    // corresponding end.
    struct {
      uint64_t interval_start_offset;
      bool start_compare_status;
      bool started_interval;
    } interval_tracker = {.started_interval = false};

    zx_status_t status = ForEveryPageAndGapInRange<PTR_TYPE>(
        self,
        [&](auto* p, uint64_t off) {
          found_page_or_gap = true;
          zx_status_t st = ZX_ERR_NEXT;
          const bool compare_result = compare_func(p, off);

          // Handle interval types first.
          if (p->IsInterval()) {
            // If we are going to start an interval, end any contiguous run being tracked, and call
            // contiguous_run_func on it. This is because intervals are treated as contiguous ranges
            // distinct from pages or markers. Do this before per_page_func because we don't want to
            // have processed extra pages if contiguous_run_func on the range prior would have
            // failed. Also do this irrespective of whether this interval passes the compare_func or
            // not, since we are processing pages prior to the interval.
            if (p->IsIntervalStart() || p->IsIntervalSlot()) {
              if (contiguous_run_len > 0) {
                st = contiguous_run_func(contiguous_run_start,
                                         contiguous_run_start + contiguous_run_len,
                                         /*is_interval=*/false);
                // Reset contiguous range tracking.
                contiguous_run_len = 0;
                if (st != ZX_ERR_NEXT) {
                  return st;
                }
              }
            }
            DEBUG_ASSERT(contiguous_run_len == 0);

            // Run the per-page function on the interval sentinel first. Then proceed to the more
            // complicated logic for the contiguous function.
            if (compare_result) {
              st = per_page_func(p, off);
              if (st != ZX_ERR_NEXT && st != ZX_ERR_STOP) {
                return st;
              }
            }

            // A slot is a contiguous run of a single page.
            if (p->IsIntervalSlot()) {
              // We should not have been already tracking an interval.
              DEBUG_ASSERT(!interval_tracker.started_interval);
              if (compare_result) {
                return contiguous_run_func(off, off + kPageSize, /*is_interval=*/true);
              }
              return ZX_ERR_NEXT;
            }

            if (p->IsIntervalStart()) {
              // Start tracking a new run. We should not have been already tracking an interval.
              DEBUG_ASSERT(!interval_tracker.started_interval);
              interval_tracker.started_interval = true;
              interval_tracker.interval_start_offset = off;
              // Stash the comparison result for the interval start.
              interval_tracker.start_compare_status = compare_result;
              return ZX_ERR_NEXT;
            }

            DEBUG_ASSERT(p->IsIntervalEnd());
            // If the interval end does not pass the check, there is nothing more to be done.
            if (!compare_result) {
              interval_tracker.started_interval = false;
              return ZX_ERR_NEXT;
            }

            // If this is the end of an interval, call contiguous_run_func on the interval if the
            // compare_func passes for *both* the start and the end, and proceed.
            // It is possible that we don't have the interval start if we started the traversal
            // partway inside an interval. Find the start and evaluate compare_func on it.
            if (!interval_tracker.started_interval) {
              auto [start, interval_start_offset] = self->FindIntervalStartForEnd(off);
              DEBUG_ASSERT(start);
              DEBUG_ASSERT(start->IsIntervalStart());
              DEBUG_ASSERT(interval_start_offset < start_offset);
              interval_tracker.started_interval = true;
              interval_tracker.start_compare_status = compare_func(start, interval_start_offset);
              // Pretend that the interval begins at start_offset since we're not considering the
              // range before it.
              interval_tracker.interval_start_offset = start_offset;
            }
            DEBUG_ASSERT(interval_tracker.started_interval);
            interval_tracker.started_interval = false;
            if (interval_tracker.start_compare_status) {
              return contiguous_run_func(interval_tracker.interval_start_offset, off + kPageSize,
                                         /*is_interval=*/true);
            }
            return ZX_ERR_NEXT;
          }

          // Handle any non-interval types.
          DEBUG_ASSERT(!p->IsInterval());
          DEBUG_ASSERT(!interval_tracker.started_interval);

          if (compare_result) {
            st = per_page_func(p, off);
            // Return any errors early before considering this page for contiguous_run_func.
            if (st != ZX_ERR_NEXT && st != ZX_ERR_STOP) {
              // If there was an outstanding contiguous run, process it since it had to have ended
              // before the failing offset.
              if (contiguous_run_len > 0) {
                zx_status_t prev_range_status = contiguous_run_func(
                    contiguous_run_start, contiguous_run_start + contiguous_run_len,
                    /*is_interval=*/false);
                contiguous_run_len = 0;
                // If there was an error encountered, surface that instead of st, as it occurred on
                // a range prior to this offset.
                if (prev_range_status != ZX_ERR_NEXT && prev_range_status != ZX_ERR_STOP) {
                  return prev_range_status;
                }
              }
              return st;
            }

            // Start tracking a contiguous run if none was being tracked.
            if (contiguous_run_len == 0) {
              contiguous_run_start = off;
            }
            // Append this page to the contiguous range being tracked.
            contiguous_run_len += kPageSize;
            // In the case that st is ZX_ERR_STOP, we will include this page in the contiguous run
            // and stop traversal *after* this page.
            return st;
          }
          // We were already tracking a contiguous range when we encountered this page that does not
          // fulfill compare_func. Invoke contiguous_run_func on the range so far and start tracking
          // a new one skipping over this page.
          if (contiguous_run_len > 0) {
            st =
                contiguous_run_func(contiguous_run_start, contiguous_run_start + contiguous_run_len,
                                    /*is_interval=*/false);
            // Reset contiguous_run_len to zero to track a new range later if required.
            // Do this irrespective of the return status to ensure we don't erroneously have a
            // remaining range to process below after exiting the traversal.
            contiguous_run_len = 0;
          }
          return st;
        },
        [&](uint64_t start, uint64_t end) {
          found_page_or_gap = true;
          // We should not encounter any gaps in the midst of an interval we were tracking.
          DEBUG_ASSERT(!interval_tracker.started_interval);
          zx_status_t st = ZX_ERR_NEXT;
          // We were already tracking a contiguous range when we encountered this gap. Invoke
          // contiguous_run_func on the range so far and start tracking a new one skipping over this
          // gap.
          if (contiguous_run_len > 0) {
            st =
                contiguous_run_func(contiguous_run_start, contiguous_run_start + contiguous_run_len,
                                    /*is_interval=*/false);
            // Reset contiguous_run_len to zero to track a new range later if required.
            // Do this irrespective of the return status to ensure we don't erroneously have a
            // remaining range to process below after exiting the traversal.
            contiguous_run_len = 0;
          }
          return st;
        },
        start_offset, end_offset);

    if (status != ZX_OK) {
      return status;
    }

    // If we did not execute either the per-page or per-gap function, we could only have been inside
    // an interval. In that case, we need to find both the start and the end of this interval and
    // evaluate compare_func on them.
    if (!found_page_or_gap) {
      DEBUG_ASSERT(self->IsOffsetInInterval(start_offset));
      DEBUG_ASSERT(self->IsOffsetInInterval(end_offset - kPageSize));

      uint64_t interval_end_offset = UINT64_MAX;
      bool end_compare_status = false;
      status = ForEveryPageInRange<PTR_TYPE>(
          self,
          [&](auto* p, uint64_t off) {
            // The first populated slot should be an interval end.
            DEBUG_ASSERT(p->IsIntervalEnd());
            interval_end_offset = off;
            end_compare_status = compare_func(p, off);
            return ZX_ERR_STOP;
          },
          end_offset, VmPageList::MAX_SIZE);
      DEBUG_ASSERT(status == ZX_OK);

      if (end_compare_status) {
        auto [start, interval_start_offset] = self->FindIntervalStartForEnd(interval_end_offset);
        DEBUG_ASSERT(start);
        DEBUG_ASSERT(start->IsIntervalStart());
        DEBUG_ASSERT(interval_start_offset < start_offset);
        if (compare_func(start, interval_start_offset)) {
          status = contiguous_run_func(start_offset, end_offset, /*is_interval=*/true);
          if (status != ZX_ERR_NEXT && status != ZX_ERR_STOP) {
            return status;
          }
        }
      }
      return ZX_OK;
    }

    // Process the last contiguous range if there is one, or an interval that we started tracking
    // but did not end.
    if (contiguous_run_len > 0) {
      status = contiguous_run_func(contiguous_run_start, contiguous_run_start + contiguous_run_len,
                                   /*is_interval=*/false);
      if (status != ZX_ERR_NEXT && status != ZX_ERR_STOP) {
        return status;
      }
    } else if (interval_tracker.started_interval && interval_tracker.start_compare_status) {
      auto [end, interval_end_offset] =
          self->FindIntervalEndForStart(interval_tracker.interval_start_offset);
      DEBUG_ASSERT(end);
      DEBUG_ASSERT(end->IsIntervalEnd());
      if (compare_func(end, interval_end_offset)) {
        status = contiguous_run_func(interval_tracker.interval_start_offset, end_offset,
                                     /*is_interval=*/true);
        if (status != ZX_ERR_NEXT && status != ZX_ERR_STOP) {
          return status;
        }
      }
    }

    return ZX_OK;
  }

  using NodeList = btree::BTree<uint64_t, VmPlnOwner>;
  NodeList list_;
};

// Class which holds the list of vm_page structs removed from a VmPageList
// by AddPagesFrom. The list include information about uncommitted pages and markers.
// Every splice list is expected to go through the following series of states:
// 1. The splice list is created.
// 2. List is Initialized with the desired range.
// 3. Pages are added to the splice list.
// 4. The list is `Finalize`d, meaning that it can no longer be modified by `Append`.
// 5. Pages are then `Pop`d from the list. Once all the pages are popped, the list is considered
//    "processed".
// 6. The list is then considered `Processed` and can be destroyed.
class VmPageSpliceList final {
 public:
  VmPageSpliceList() = default;
  // Convenience constructor that calls Initialize.
  explicit VmPageSpliceList(uint64_t length) { Initialize(length); }
  ~VmPageSpliceList();

  // For use by PhysicalPageProvider.  The user-pager path doesn't use this. This returns a
  // finalized list.
  static zx_status_t CreateFromPageList(uint64_t length, VmPageDoublyLinkedList* pages,
                                        VmPageSpliceList* splice);

  // Initialize the list with the given range. Can only be done once, and must be done before
  // providing pages.
  void Initialize(uint64_t length) {
    DEBUG_ASSERT(IsEmpty() && state_ == State::Constructed);
    length_ = length;
    state_ = State::Initialized;
  }

  // Pops the next page off of the splice list. It is invalid to pop a page from a non-finalized
  // splice list.
  VmPageOrMarker Pop();

  // Takes the content out of |source| and places them in this splice list, which must have already
  // been initialized but still be empty. The range to be taken is determined by the provided
  // |offset| and the length of this splice list. May return ZX_ERR_OUT_OF_MEMORY, in which case an
  // unspecified number of pages will have been moved into this splice list.
  //
  // Takes a function with the signature void(VmPageOrMarker *src, VmPageOrMarker *dst, uint64_t
  // offset), which is expected to move content from |src| to |dst|.
  template <typename F>
  zx_status_t AddPagesFrom(F merge_func, VmPageList& source, uint64_t offset) {
    const uint64_t end = offset + length_;
    bool result = source.MergeRangeOnto(merge_func, page_list_, offset, end);
    Finalize();
    return result ? ZX_OK : ZX_ERR_NO_MEMORY;
  }

  // Iterates all pages in this splice list without taking ownership. Pages are not considered
  // processed and can not be removed using this iterator. The callback is expected to have the
  // signature: zx_status_t page_func(VmPageOrMarkerRef slot, uint64_t offset);
  // The iterator will start at `start_offset` and iterate until the end of the VmPageSpliceList.
  // start_offset must be paged aligned and less than length_.
  template <typename PAGE_FUNC>
  zx_status_t MutatePages(PAGE_FUNC page_func, uint64_t start_offset) {
    DEBUG_ASSERT(IsFinalized());
    DEBUG_ASSERT(start_offset < length_);
    DEBUG_ASSERT(IsPageRounded(start_offset));
    zx_status_t status = page_list_.ForEveryPageInRangeMutable(
        [&](VmPageOrMarkerRef slot, uint64_t offset) { return page_func(slot, offset); },
        start_offset, length_);

    return status;
  }

  // Iterates all the pages and gaps in this splice list. The page_func is given ownership of each
  // page. It is invalid to process a non-finalized splice list. The two callbacks are expected to
  // have the signature:
  // zx_status_t page_func(VmPageOrMarker slot, uint64_t splice_offset);
  // zx_status_t gap_func(uint64_t gap_start, uint64_t gap_end);
  // Due to page_func always taking ownership of the content if it returns any kind of error, either
  // a graceful request to stop iteration via ZX_ERR_STOP or an explicit error, then that offset is
  // considered processed and will not be repeated in future calls. In contrast, if iteration is
  // stopped during gap_func, that gap will not be considered processed and can be returned in
  // future calls.
  template <typename PAGE_FUNC, typename GAP_FUNC>
  zx_status_t RemovePagesAndIterateGaps(PAGE_FUNC page_func, GAP_FUNC gap_func) {
    DEBUG_ASSERT(IsFinalized());
    // Assume we will successfully process the whole range. This will get trimmed should any of the
    // callbacks terminate early.
    uint64_t processed = length_;
    zx_status_t status = page_list_.RemovePagesAndIterateGaps(
        [&](VmPageOrMarker* slot, uint64_t src_offset) {
          // Move the content out of slot before passing to the page_func to ensure it *must* deal
          // with it and it cannot be left in slot.
          VmPageOrMarker content = ktl::move(*slot);
          zx_status_t status = page_func(ktl::move(content), src_offset);
          if (status != ZX_ERR_NEXT) {
            processed = src_offset + kPageSize;
          }
          return status;
        },
        [&](uint64_t gap_start, uint64_t gap_end) {
          zx_status_t status = gap_func(gap_start, gap_end);
          if (status != ZX_ERR_NEXT) {
            processed = gap_start;
          }
          return status;
        },
        pos_, length_);
    pos_ = processed;
    if (pos_ == length_) {
      DEBUG_ASSERT(page_list_.IsEmpty());
      state_ = State::Processed;
    }
    return status;
  }

  // Inserts the |content| into the splice list at the specified |offset|.
  // The splice list takes ownership of `content` after this call.
  // It is invalid to append to a finalized splice list.
  zx_status_t Insert(uint64_t offset, VmPageOrMarker content);

  // Returns true after the whole collection has been processed by Pop.
  bool IsProcessed() const { return state_ == State::Processed; }

  // Returns true if the collection has been Initialized.
  bool IsInitialized() const { return state_ == State::Initialized; }

  // Returns true if this list is empty.
  bool IsEmpty() const { return page_list_.IsEmpty(); }

  // Marks the list as finalized.
  // See the comment at `VmPageSpliceList`'s declaration for more info on what this means and when
  // to call it. Note that it is invalid to call `Finalize` twice on the same list.
  void Finalize() {
    DEBUG_ASSERT(IsInitialized());
    state_ = State::Finalized;
  }

  // Returns true if the splice list is finalized.
  // See the comment at `VmPageSpliceList`'s declaration for more info on what this means.
  bool IsFinalized() const { return state_ == State::Finalized; }

  // Returns the current position in the list.
  uint64_t Position() const { return pos_; }

  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(VmPageSpliceList);

 private:
  void FreeAllPages();

  uint64_t length_ = 0;
  uint64_t pos_ = 0;

  // States used to represent the life cycle of the VmPageSpliceList, as per the comment at the
  // declaration.
  enum class State : uint8_t {
    Constructed,
    Initialized,
    Finalized,
    Processed,
  };
  State state_ = State::Constructed;

  VmPageList page_list_;

  friend VmPageList;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_PAGE_LIST_H_
