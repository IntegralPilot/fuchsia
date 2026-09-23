// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_COW_PAGES_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_COW_PAGES_H_

#include <assert.h>
#include <lib/page/size.h>
#include <lib/user_copy/user_ptr.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>
#include <zircon/types.h>

#include <fbl/array.h>
#include <fbl/canary.h>
#include <fbl/enum_bits.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/macros.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/mutex.h>
#include <vm/compressor.h>
#include <vm/continuous_attribution_tracker.h>
#include <vm/page_source.h>
#include <vm/physical_page_borrowing_config.h>
#include <vm/pmm.h>
#include <vm/pmm_node.h>
#include <vm/stream_size_manager.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object.h>
#include <vm/vm_page_list.h>

// Forward declare these so VmCowPages helpers can accept references.
class BatchPQRemove;
class VmObjectPaged;
class DiscardableVmoTracker;
class PriorityChanger;

enum class VmCowPagesOptions : uint32_t {
  // Externally-usable flags:
  kNone = 0u,
  kUserPagerBackedRoot = (1u << 0),
  kPageSourceRoot = (1u << 1),

  // With this clear, zeroing a page tries to decommit the page.  With this set, zeroing never
  // decommits the page.  Currently this is only set for contiguous VMOs.
  //
  // TODO(dustingreen): Once we're happy with the reliability of page borrowing, we should be able
  // to relax this restriction.  We may still need to flush zeroes to RAM during reclaim to mitigate
  // a hypothetical client incorrectly assuming that cache-clean status will remain intact while
  // pages aren't pinned, but that mitigation should be sufficient (even assuming such a client) to
  // allow implicit decommit when zeroing or when zero scanning, as long as no clients are doing DMA
  // to/from contiguous while not pinned.
  kCannotDecommitZeroPages = (1u << 2),

  // Internal-only flags:
  kHidden = (1u << 3),

  kInternalOnlyMask = kHidden,
};
FBL_ENABLE_ENUM_BITS(VmCowPagesOptions)

struct VmCowRange {
  uint64_t offset;
  uint64_t len;

  constexpr VmCowRange() : offset(0), len(0) {}
  constexpr VmCowRange(uint64_t offset, uint64_t len) : offset(offset), len(len) {}

  uint64_t end() const { return offset + len; }
  bool is_empty() const { return len == 0; }
  bool is_page_aligned() const { return IsPageRounded(offset) && IsPageRounded(len); }
  VmCowRange ExpandTillPageAligned() const {
    const uint64_t start = RoundDownPageSize(offset);
    return VmCowRange(start, RoundUpPageSize(end()) - start);
  }

  VmCowRange OffsetBy(uint64_t delta) const { return VmCowRange(offset + delta, len); }
  VmCowRange TrimmedFromStart(uint64_t amount) const {
    return VmCowRange(offset + amount, len - amount);
  }
  // Returns the minimal range that covers both |this| and |other|. If these ranges are disjoint
  // then the returned range will be larger than combined length of |this| and |other| in order to
  // span both using a single range.
  VmCowRange Cover(VmCowRange other) const {
    if (is_empty()) {
      return other;
    }
    if (other.is_empty()) {
      return *this;
    }
    const uint64_t start = ktl::min(offset, other.offset);
    const uint64_t end = ktl::max(offset + len, other.offset + other.len);
    return VmCowRange(start, end - start);
  }
  VmCowRange WithLength(uint64_t new_length) const { return VmCowRange(offset, new_length); }
  bool IsBoundedBy(uint64_t max) const;
};

// Result of attempting to reclaim a single page. See VmCowPages::ReclaimPage for more details.
struct VmCowReclaimSuccess {
  enum class Type : uint8_t {
    Evict,
    Discard,
    Compress,
  } type;
  uint64_t num_pages = 0;
  uint64_t num_loaned_pages = 0;
};
enum class VmCowReclaimFailure : uint8_t {
  EvictAccessed,
  CompressAccessed,
  CompressFailed,
  IncorrectPage,
  Other,
};

enum class PageSourceType : uint8_t {
  // The VmCowPages is not directly backed by a page_source_.
  Anonymous = 0,
  // The VmCowPages is directly backed by a contiguous page source. That has the following
  // implications:
  // * The direct page source provides specific physical pages.
  // * The direct page source (possibly with the help of the VmCowPages) provides zeroed pages.
  Contiguous,
  // The VmCowPages is directly backed by a user pager. That has the following implications:
  // * The direct page source is responsible for freeing the page content.
  // * The VmCowPages must implement dirty tracking.
  UserPager,
};

#if EXPERIMENTAL_CONTINUOUS_PER_VMO_ATTRIBUTION_ENABLED
using AttributionTracker = ContinuousAttributionTracker;
#else   // EXPERIMENTAL_CONTINUOUS_PER_VMO_ATTRIBUTION_ENABLED
using AttributionTracker = StubContinuousAttributionTracker;
#endif  // EXPERIMENTAL_CONTINUOUS_PER_VMO_ATTRIBUTION_ENABLED

namespace page_cache {
class PageCache;
}  // namespace page_cache

class ScopedPageFreedList;

extern "C" void cpp_vm_cow_pages_free(VmCowPages*);

// Implements a copy-on-write hierarchy of pages in a VmPageList.
// VmCowPages have a life cycle where they start in an Init state to allow them to have
// initialization finished outside the constructor. A VmCowPages in the Init state may be
// destructed, although it is not allowed to have any pages put in it.
// Once transitioned to the Alive state the VmCowPages may generally be used, and must be
// explicitly transitioned to the Dead state prior to being destructed. The explicit transition
// ensures that a VmCowPages does not own any pages whilst in its destructor, and hence while the
// object is unreachable due to having a ref count of 0.
class VmCowPages final : public fbl::ContainableBaseClasses<
                             fbl::TaggedDoublyLinkedListable<VmCowPages*, internal::ChildListTag>>,
                         public fbl::RefCountedUpgradeable<VmCowPages> {
 public:
  static zx_status_t Create(VmCowPagesOptions options, uint32_t pmm_alloc_flags, uint64_t size,
                            ktl::unique_ptr<DiscardableVmoTracker> discardable_tracker,
                            fbl::RefPtr<VmCowPages>* cow_pages);

  static zx_status_t CreateExternal(fbl::RefPtr<PageSource> src, VmCowPagesOptions options,
                                    uint64_t size, fbl::RefPtr<VmCowPages>* cow_pages);

  // Define the lock retrieval functions differently depending on whether we should be returning a
  // local lock instance, or the common one in the hierarchy_state_ptr. Due to the TA_RET_CAP
  // statements we cannot perform |if constexpr| or equivalent indirection in the function body, and
  // must have two completely different function definitions.
  // In the absence of a local lock it is assumed, and enforced in vm_object_lock.h, that there is a
  // shared lock in the hierarchy state. If there is both a local and a shared lock then the local
  // lock is to be used for the improved lock tracking.
  Lock<CriticalMutex>* lock() const TA_RET_CAP(lock_) { return &lock_; }
  Lock<CriticalMutex>& lock_ref() const TA_RET_CAP(lock_) { return lock_; }

  uint64_t lock_order() const {
#if (LOCK_DEP_ENABLED_FEATURE_LEVEL > 0)
    return lock_order_;
#endif
    // When the lock order isn't in use just return a garbage value, whatever is calculated using it
    // will get thrown away regardless.
    return 0;
  }

  // Similar to LockedPtr, but holds a RefPtr instead of a raw pointer.
  class LockedRefPtr {
   public:
    LockedRefPtr() = default;
    ~LockedRefPtr() { release(); }
    LockedRefPtr(LockedRefPtr&& l) = default;
    explicit LockedRefPtr(const fbl::RefPtr<VmCowPages>& object)
        : LockedRefPtr(object, object->lock_order()) {}
    LockedRefPtr(fbl::RefPtr<VmCowPages> object, uint64_t lock_order)
        : ptr_(ktl::move(object)),
          lock_(Guard<CriticalMutex>(AssertOrderedLock, ptr_->lock(), lock_order).take()) {}
    ktl::pair<fbl::RefPtr<VmCowPages>, Guard<CriticalMutex>::Adoptable> take() {
      return {ktl::move(ptr_), ktl::move(lock_)};
    }

    VmCowPages& locked() const TA_ASSERT(locked().lock()) { return *ptr_; }

    fbl::RefPtr<VmCowPages>&& release() {
      if (ptr_) {
        Guard<CriticalMutex> guard{AdoptLock, ptr_->lock(), ktl::move(lock_)};
      }
      return ktl::move(ptr_);
    }

    VmCowPages* get() const { return ptr_.get(); }
    VmCowPages& operator*() const { return *ptr_; }
    VmCowPages* operator->() const { return get(); }

    explicit operator bool() const { return !!ptr_; }
    LockedRefPtr& operator=(LockedRefPtr&& other) {
      release();
      ptr_ = ktl::move(other.ptr_);
      lock_ = ktl::move(other.lock_);
      return *this;
    }

   private:
    fbl::RefPtr<VmCowPages> ptr_;
    Guard<CriticalMutex>::Adoptable lock_;
  };
  class DeferredOps;

  // Creates a copy-on-write clone with the desired parameters. This can fail due to various
  // internal states not being correct.
  zx::result<LockedRefPtr> CreateCloneLocked(SnapshotType type, bool require_unidirection,
                                             VmCowRange range, DeferredOps& ops) TA_REQ(lock());

  // VmCowPages are initially created in the Init state and need to be transitioned to Alive prior
  // to being used. This is exposed for VmObjectPaged to call after ensuring that creation is
  // successful, i.e. after it can guarantee that it will transition this cow pages to Dead prior to
  // it being destroyed.
  void TransitionToAliveLocked() TA_REQ(lock());

  // Returns the size in bytes of this cow pages range. This will always be a multiple of the page
  // size.
  uint64_t size_locked() const TA_REQ(lock()) { return size_; }

  // Returns whether this cow pages node is ultimately backed by a user pager to fulfill initial
  // content, and not zero pages.  Contiguous VMOs have page_source_ set, but are not pager backed
  // in this sense.
  //
  // This should only be used to report to user mode whether a VMO is user-pager backed, not for any
  // other purpose.
  bool is_root_source_user_pager_backed() const {
    return !!(options_ & VmCowPagesOptions::kUserPagerBackedRoot);
  }

  // Returns whether the root of the cow pages hierarchy has non-null page_source_.
  bool root_has_page_source() const { return !!(options_ & VmCowPagesOptions::kPageSourceRoot); }

  // Helper function for CowPage cloning methods. Returns any options that should be passed down to
  // the child.
  VmCowPagesOptions inheritable_options() const {
    return VmCowPagesOptions::kNone | (options_ & (VmCowPagesOptions::kUserPagerBackedRoot |
                                                   VmCowPagesOptions::kPageSourceRoot));
  }

  bool is_root_source_preserving_page_content() const {
    return !!(options_ & VmCowPagesOptions::kUserPagerBackedRoot);
  }

  bool is_parent_hidden_locked() const TA_REQ(lock()) { return parent_ && parent_->is_hidden(); }

  bool has_no_children_locked() const TA_REQ(lock()) { return children_list_len_ == 0; }

  bool is_discardable() const { return !!discardable_tracker_; }

  bool can_evict() const { return page_source_ && page_source_->properties().is_user_pager; }

  bool can_root_source_evict() const {
    bool result = is_root_source_preserving_page_content();
    DEBUG_ASSERT(result == is_root_source_user_pager_backed());
    return result;
  }

  // can_borrow() returns true if the VmCowPages is capable of borrowing pages, but whether
  // the VmCowPages should actually borrow pages also depends on a borrowing-site-specific flag that
  // the caller is responsible for checking (in addition to checking can_borrow()).  Only if
  // both are true should the caller actually borrow at the caller's specific potential borrowing
  // site.
  // Aside from the general borrowing in the PhysicalPageBorrowingConfig being turned on and
  // off, the ability to borrow is constant over the lifetime of the VmCowPages.
  bool can_borrow() const {
    // TODO(dustingreen): Or rashaeqbal@.  We can only borrow while the page is not dirty.
    // Currently we enforce this by checking ShouldTrapDirtyTransitions() below and leaning on the
    // fact that !ShouldTrapDirtyTransitions() dirtying isn't implemented yet.  We currently evict
    // to reclaim instead of replacing the page, and we can't evict a dirty page since the contents
    // would be lost.  Option 1: When a loaned page is about to become dirty, we could replace it
    // with a non-loaned page.  Option 2: When reclaiming a loaned page we could replace instead of
    // evicting (this may be simpler).

    // Currently there needs to be a page source for any borrowing to be possible, due to
    // requirements of a backlink and other assumptions in the VMO code. Returning early here in the
    // absence of a page source simplifies the rest of the logic.
    if (!page_source_) {
      return false;
    }

    bool source_is_suitable = page_source_->properties().is_user_pager;

    // Avoid borrowing and trapping dirty transitions overlapping for now; nothing really stops
    // these from being compatible AFAICT - we're just avoiding overlap of these two things until
    // later.
    bool overlapping_with_other_features = page_source_->ShouldTrapDirtyTransitions();

    return source_is_suitable && !overlapping_with_other_features;
  }

  // In addition to whether a VmCowPages is allowed, for correctness reasons, to borrow pages there
  // are other, potentially variable, factors that influence whether it's considered a good idea for
  // this VmCowPages to borrow pages. In particular it's possible for this to change over the
  // lifetime of the VmCowPages.
  bool should_borrow_locked() const TA_REQ(lock()) {
    if (!can_borrow()) {
      return false;
    }
    // Exclude is_latency_sensitive_ to avoid adding latency due to reclaim.
    //
    // Currently we evict instead of replacing a page when reclaiming, so we want to avoid evicting
    // pages that are latency sensitive or are fairly likely to be pinned at some point.
    //
    // We also don't want to borrow a page that might get pinned again since we want to mitigate the
    // possibility of an invalid DMA-after-free.
    const bool excluded_from_borrowing_for_latency_reasons =
        high_priority_count_ != 0 || ever_pinned_.load(ktl::memory_order_relaxed);
    return !excluded_from_borrowing_for_latency_reasons;
  }

  // Returns whether this cow pages node is dirty tracked.
  bool is_dirty_tracked() const {
    // Pager-backed VMOs require dirty tracking either if they are directly backed by the pager,
    // i.e. the root VMO.
    return page_source_ && page_source_->properties().is_user_pager;
  }

  // If true this node, and all nodes in this hierarchy, are using parent content markers to
  // indicate when a leaf node may need to walk up the tree to find content.
  //
  // When parent content markers are in use an empty page list slot in a leaf node means that there
  // is *no* visible parent content above, and the parent hierarchy does not have to be searched.
  //
  // For memory efficiency, and because it would be redundant, parent content markers are never
  // placed in the hidden nodes, only leaf nodes.
  //
  // The presence of a parent content marker in a leaf node indicates that there *might* be content
  // in a parent node and that a tree walk *must* be performed to search for it. The reason for
  // spurious parent content markers is that zero page deduplication could happen on hidden nodes,
  // which could remove the content, but leave the parent content markers in the leaf nodes.
  // Spurious parent content markers can persist in VMOs when they lose their parent. Spurious
  // parent content markers are redundant and could be cleaned up.
  //
  // Use of parent content markers is just the inverse of having a page source, since if there is a
  // page source we always have to go to it for content as the zero page cannot be assumed. Although
  // some page sources do supply zero content (physical page provider for contiguous VMOs),
  // optimizing this check for that is redundant since such page sources do not support
  // copy-on-write, and so never have children to begin with.
  bool tree_has_parent_content_markers() const { return !root_has_page_source(); }

  // Indicates whether this node can have parent content markers placed in it. This is just checking
  // if it is both a leaf node, and the tree overall can have parent content markers.
  //
  // Note that even if this is false, if |tree_has_parent_content_markers| is true then reasoning
  // may need to be done about parent content markers.
  bool node_has_parent_content_markers() const {
    return !is_hidden() && tree_has_parent_content_markers();
  }

  // The modified state is only supported for root pager-backed VMOs, and will get queried (and
  // possibly reset) on the next QueryPagerVmoStatsLocked() call. Although the modified state is
  // only tracked for the root VMO.
  void mark_modified_locked() TA_REQ(lock()) {
    if (!is_dirty_tracked()) {
      return;
    }
    DEBUG_ASSERT(page_source_type() == PageSourceType::UserPager);
    pager_stats_modified_ = true;
  }

  bool is_high_memory_priority_locked() const TA_REQ(lock()) {
    DEBUG_ASSERT(high_priority_count_ >= 0);
    return high_priority_count_ != 0;
  }

  // See description on |pinned_page_count_| for meaning.
  uint64_t pinned_page_count_locked() const TA_REQ(lock()) { return pinned_page_count_; }

  // Sets the VmObjectPaged backlink for this copy-on-write node.
  // Currently it is assumed that all nodes always have backlinks with the 1:1 hierarchy mapping,
  // unless this is a hidden node.
  void set_paged_backlink_locked(VmObjectPaged* ref) TA_REQ(lock()) { paged_ref_ = ref; }

  VmObjectPaged* get_paged_backlink_locked() const TA_REQ(lock()) { return paged_ref_; }

  uint64_t HeapAllocationBytesLocked() const TA_REQ(lock()) {
    return page_list_.HeapAllocationBytes();
  }

  uint64_t ReclamationEventCountLocked() const TA_REQ(lock()) { return reclamation_event_count_; }

  void DetachSource();

  ktl::optional<zx_koid_t> GetPageSourceKoid() const {
    if (!page_source_) {
      return ktl::nullopt;
    }
    return page_source_->GetProviderKoid();
  }

  // Resizes the range of this cow pages. |size| must be a multiple of the page size.
  zx_status_t Resize(uint64_t size);

  // See VmObject::Lookup
  zx_status_t LookupLocked(VmCowRange range, VmObject::LookupFunction lookup_fn) TA_REQ(lock());

  // Similar to LookupLocked, but enumerate all readable pages in the hierarchy within the requested
  // range. The offset passed to the |lookup_fn| is the offset this page is visible at in this
  // object, even if the page itself is committed in a parent object. The physical addresses given
  // to the lookup_fn should not be retained in any way unless the range has also been pinned by the
  // caller.
  // Ranges of length zero are considered invalid and will return ZX_ERR_INVALID_ARGS. The lookup_fn
  // can terminate iteration early by returning ZX_ERR_STOP.
  using LookupReadableFunction =
      fit::inline_function<zx_status_t(uint64_t offset, paddr_t pa), 4 * sizeof(void*)>;
  zx_status_t LookupReadableLocked(VmCowRange range, LookupReadableFunction lookup_fn)
      TA_REQ(lock());

  // This function merely calls |LookupReadableLocked| with the lock held. Note that there is no way
  // non-test code can use this function in a correct way due to its internal locking.
  zx_status_t DebugLookupReadable(VmCowRange range, LookupReadableFunction lookup_fn)
      TA_EXCL(lock());

  // See VmObject::TakePages
  //
  // May return ZX_ERR_SHOULD_WAIT if the |page_request| is filled out and needs waiting on. In this
  // case |taken_len| might be populated with a value less than |len|.
  //
  // |taken_len| is always filled with the amount of |len| that has been processed to allow for
  // gradual progress of calls. Will always be equal to |len| if ZX_OK is returned. Similarly the
  // |splice_offset| indicates the base offset in |pages| where the content should be inserted.
  zx_status_t TakePages(VmCowRange range, uint64_t splice_offset, VmPageSpliceList* pages,
                        uint64_t* taken_len, MultiPageRequest* page_request);

  // Pre-process a VmPageSpliceList for supply.
  //
  // If there is a page source, references are converted to pages. When allocating pages from
  // references, ZX_ERR_SHOULD_WAIT can be returned which is managed by this function.
  // TODO(sagebarred): handle InitializeVmPage from PhysicalPageProvider here.
  zx_status_t ProcessPagesForSupply(VmPageSpliceList* pages);

  // See VmObject::SupplyPages
  //
  // Should never return ZX_ERR_SHOULD_WAIT, waiting on page requests is managed by
  // ProcessPagesForSupply.
  zx_status_t SupplyPagesLocked(VmCowRange range, VmPageSpliceList* pages, SupplyOptions options,
                                DeferredOps& deferred, MultiPageRequest* page_request)
      TA_REQ(lock());

  // See VmObject::FailPageRequests
  zx_status_t FailPageRequestsLocked(VmCowRange range, zx_status_t error_status) TA_REQ(lock());

  // Used to track dirty_state in the vm_page_t.
  //
  // The transitions between the three states can roughly be summarized as follows:
  // 1. A page starts off as Clean when supplied.
  // 2. A write transitions the page from Clean to Dirty.
  // 3. A writeback_begin moves the Dirty page to AwaitingClean.
  // 4. A writeback_end moves the AwaitingClean page to Clean.
  // 5. A write that comes in while the writeback is in progress (i.e. the page is AwaitingClean)
  // moves the AwaitingClean page back to Dirty.
  enum class DirtyState : uint8_t {
    // The page does not track dirty state. Used for non pager backed pages.
    Untracked = 0,
    // The page is clean, i.e. its contents have not been altered from when the page was supplied.
    Clean,
    // The page's contents have been modified from the time of supply, and should be written back to
    // the page source at some point.
    Dirty,
    // The page still has modified contents, but the page source is in the process of writing back
    // the changes. This is used to ensure that a consistent version is written back, and that any
    // new modifications that happen during the writeback are not lost. The page source will mark
    // pages AwaitingClean before starting any writeback.
    AwaitingClean,
    NumStates,
  };
  // Make sure that the state can be encoded in the vm_page_t's dirty_state field.
  static_assert(static_cast<uint8_t>(DirtyState::NumStates) <= VM_PAGE_OBJECT_MAX_DIRTY_STATES);

  static bool is_page_dirty_tracked(const vm_page_t* page) {
    return DirtyState(page->object.dirty_state) != DirtyState::Untracked;
  }
  static bool is_page_dirty(const vm_page_t* page) {
    return DirtyState(page->object.dirty_state) == DirtyState::Dirty;
  }
  static bool is_page_clean(const vm_page_t* page) {
    return DirtyState(page->object.dirty_state) == DirtyState::Clean;
  }
  static bool is_page_awaiting_clean(const vm_page_t* page) {
    return DirtyState(page->object.dirty_state) == DirtyState::AwaitingClean;
  }
  static bool is_page_reclaimable(const vm_page_t* page) {
    return PageQueues::IsPageReclaimable(page);
  }

  // See VmObject::DirtyPages. |page_request| is required to support delayed PMM allocations; if
  // ZX_ERR_SHOULD_WAIT is returned the caller should wait on |page_request|. |alloc_list| will hold
  // any pages that were allocated but not used in case of delayed PMM allocations, so that it can
  // be reused across multiple successive calls whilst ensuring forward progress.
  zx_status_t DirtyPages(VmCowRange range, VmPageDoublyLinkedList* alloc_list,
                         AnonymousPageRequest* page_request);

  using DirtyRangeEnumerateFunction = VmObject::DirtyRangeEnumerateFunction;
  // See VmObject::EnumerateDirtyRanges
  zx_status_t EnumerateDirtyRangesLocked(VmCowRange range,
                                         DirtyRangeEnumerateFunction&& dirty_range_fn)
      TA_REQ(lock());

  // Query pager VMO |stats|, and reset them too if |reset| is set to true.
  zx_status_t QueryPagerVmoStatsLocked(bool reset, zx_pager_vmo_stats_t* stats) TA_REQ(lock()) {
    canary_.Assert();
    DEBUG_ASSERT(stats);
    // The modified state should only be set for VMOs directly backed by a pager.
    DEBUG_ASSERT(!pager_stats_modified_ || page_source_type() == PageSourceType::UserPager);

    if (page_source_type() != PageSourceType::UserPager) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    stats->modified = pager_stats_modified_ ? ZX_PAGER_VMO_STATS_MODIFIED : 0;
    if (reset) {
      ResetPagerVmoStatsLocked();
    }
    return ZX_OK;
  }

  void ResetPagerVmoStatsLocked() TA_REQ(lock()) { pager_stats_modified_ = false; }

  // See VmObject::WritebackBegin
  zx_status_t WritebackBeginLocked(VmCowRange range, bool is_zero_range) TA_REQ(lock());

  // See VmObject::WritebackEnd
  zx_status_t WritebackEndLocked(VmCowRange range) TA_REQ(lock());

  // Tries to prepare the range [offset, offset + len) for writing by marking pages dirty or
  // verifying that they are already dirty. It is possible for only some or none of the pages in the
  // range to be dirtied at the end of this call. |dirty_len_out| will return the (page-aligned)
  // length starting at |offset| that contains dirty pages, either already dirty before making the
  // call or dirtied during the call. In other words, the range [offset, offset + dirty_len_out)
  // will be dirty when this call returns, i.e. prepared for the write to proceed, where
  // |dirty_len_out| <= |len|.
  //
  // If the specified range starts with pages that are not already dirty and need to request the
  // page source before transitioning to dirty, a DIRTY page request will be forwarded to the page
  // source. In this case |dirty_len_out| will be set to 0, ZX_ERR_SHOULD_WAIT will be returned and
  // the caller should wait on |page_request|. If no page requests need to be generated, i.e. we
  // could find some pages that are already dirty at the start of the range, or if the VMO does not
  // require dirty transitions to be trapped, ZX_OK is returned.
  //
  // |offset| and |len| should be page-aligned.
  zx_status_t PrepareForWriteLocked(VmCowRange range, LazyPageRequest* page_request,
                                    uint64_t* dirty_len_out) TA_REQ(lock());

  class LookupCursor;
  // See VmObjectPaged::GetLookupCursorLocked
  zx::result<LookupCursor> GetLookupCursorLocked(VmCowRange range) TA_REQ(lock());

  // Controls the type of VmPageOrMarker slot in |this| VmCowPages' page_list_ that can be
  // overwritten by the Add[New]Page[s]Locked functions. It is the caller's responsibility to ensure
  // that the previous content is dealt with correctly (e.g. any pages and compressed
  // references are freed).
  enum class CanOverwriteSlot : uint8_t {
    // Only overwrite empty slots.
    Empty,
    // Can overwrite parent content markers (in addition to empty slots).
    EmptyOrParent,
    // Can overwrite zero page markers and zero intervals (in addition to empty slots and
    // parent content markers).
    ZeroMarkerOrInterval,
    // Can overwrite pages and compressed references (in addition to empty slots, parent content
    // markers, zero markers and intervals), i.e. can overwrite all VmPageOrMarker types.
    PageOrRef,
  };
  // Adds an allocated page to this cow pages at the specified offset, can be optionally zeroed and
  // any mappings invalidated. If an error is returned the caller retains ownership of |page|.
  // Offset must be page aligned. Mappings being invalidated is controlled by |deferred| where if
  // a nullptr is passed then no mappings, in this object or any child, will be invalidated. If
  // |deferred| is non-null then mappings will be invalidated if necessary, both immediately in this
  // object, and via |deferred| for the children.
  //
  // |overwrite| controls how the function handles pre-existing content at |offset|. If |overwrite|
  // does not permit replacing the content, ZX_ERR_ALREADY_EXISTS will be returned. If a page is
  // released from the page list as a result of overwriting, it is returned through |released_page|
  // and the caller takes ownership of this page. If the |overwrite| action is such that a page
  // cannot be released, it is valid for the caller to pass in nullptr for |released_page|.
  zx_status_t AddNewPageLocked(uint64_t offset, vm_page_t* page, CanOverwriteSlot overwrite,
                               VmPageOrMarker* released_page, bool zero, DeferredOps* deferred)
      TA_REQ(lock());

  // Adds a set of pages consecutively starting from the given offset. Regardless of the return
  // result ownership of the pages is taken. Pages are assumed to be in the ALLOC state and can be
  // optionally zeroed before inserting. start_offset must be page aligned.
  //
  // |overwrite| controls how the function handles pre-existing content in the range, however it is
  // not valid to specify the |CanOverwriteSlot::PageOrRef| option, as any pages or compressed
  // references that would get released as a consequence cannot be returned.
  zx_status_t AddNewPagesLocked(uint64_t start_offset, VmPageDoublyLinkedList* pages,
                                CanOverwriteSlot overwrite, bool zero, DeferredOps* deferred)
      TA_REQ(lock());

  // Attempts to release pages in the pages list causing the range to become copy-on-write again.
  // For consistency if there is a parent or a backing page source, such that the range would not
  // explicitly copy-on-write the zero page then this will fail. Use ZeroPagesLocked for an
  // operation that is guaranteed to succeed, but may not release memory.
  zx_status_t DecommitRange(VmCowRange range);

  // After successful completion the range of pages will all read as zeros. The mechanism used to
  // achieve this is not guaranteed to decommit, but it will try to.
  // |range| must be page aligned offsets within the range of the object. |dirty_track| specifies
  // whether the range being zeroed subscribes to dirty tracking, if |true| the range will start out
  // as dirty. |dirty_track| only has meaning if the VMO supports dirty tracking, otherwise it is
  // ignored.
  //
  // Returns a status code, and the number of bytes that were actually zeroed.
  // This may be nonzero even if the returned status != ZX_OK.
  //
  // The returned status is one of the following:
  //  * ZX_OK => The whole range was successfully zeroed.
  //  * ZX_ERR_SHOULD_WAIT => The caller needs to wait on the |page_request| and then retry the
  //    operation. The caller may advance the start offset by the number of zeroed bytes before
  //    retrying.
  //  * Any other error code indicates a failure to zero a part of the range or the whole
  //  range.
  ktl::pair<zx_status_t, uint64_t> ZeroPagesLocked(VmCowRange range, bool dirty_track,
                                                   DeferredOps& deferred,
                                                   MultiPageRequest* page_request) TA_REQ(lock());

  // Attempts to commit a range of pages. This has three kinds of return status
  //  ZX_OK => The whole range was successfully committed and |len| will be written to
  //           |committed_len|
  //  ZX_ERR_SHOULD_WAIT => A partial (potentially 0) range was committed (output in |committed_len|
  //                        and the passed in |page_request| should be waited on before retrying
  //                        the commit operation. The portion that was successfully committed does
  //                        not need to retried.
  //  * => Any other error, the number of pages committed is undefined.
  // The |offset| and |len| are assumed to be page aligned and within the range of |size_|.
  zx_status_t CommitRangeLocked(VmCowRange range, DeferredOps& deferred, uint64_t* committed_len,
                                MultiPageRequest* page_request) TA_REQ(lock());

  // Increases the pin count of the range of pages given by |offset| and |len|. The full range must
  // already be committed and this either pins all pages in the range, or pins no pages and returns
  // an error. The caller can assume that on success len / kPageSize pages were pinned.
  // The |offset| and |len| are assumed to be page aligned and within the range of |size_|.
  // All pages in the specified range are assumed to be non-loaned pages, so the caller is expected
  // to replace any loaned pages beforehand if required.
  zx_status_t PinRangeLocked(VmCowRange range) TA_REQ(lock());

  // See VmObject::Unpin
  // An optional |DeferredOps| can be provided for the purposes of performing extra debugging
  // checks, but otherwise has no functional requirement. The debug checks are optional as call
  // sites may not be able to satisfy the locking requirements to construct a DeferredOps, and may
  // know (due to be undoing a pin they themselves had started), that no checks need to be done.
  void UnpinLocked(VmCowRange range, DeferredOps* deferred) TA_REQ(lock());

  // See VmObject::DebugIsRangePinned
  bool DebugIsRangePinnedLocked(VmCowRange range) TA_REQ(lock());

  // Returns true if a page is not currently committed, and if the offset were to be read from, it
  // would be read as zero. Requested offset must be page aligned and within range.
  bool PageWouldReadZeroLocked(uint64_t page_offset) TA_REQ(lock());

  // see VmObject::GetAttributedMemoryInRange
  using AttributionCounts = VmObject::AttributionCounts;
  AttributionCounts GetAttributedMemoryInRangeLocked(VmCowRange range) const TA_REQ(lock());

  enum class EvictionAction : uint8_t {
    FollowHint,
    IgnoreHint,
  };

  // Asks the VMO to attempt to reclaim the specified page. There are a few possible outcomes:
  // 1. Exactly this page is reclaimed.
  // 2. This page and other pages are reclaimed.
  // 3. Just other pages are reclaimed.
  // 4. No pages are reclaimed.
  // Pages other than the one requested may get reclaimed due to any internal relationships between
  // pages that make it meaningless or difficult to reclaim just the single page in question.
  // In the cases of (3) and (4) there are some guarantees provided:
  // 1. If the |page| was not from this VMO (or not at the specified offset) then nothing about the
  //    |page| or this VMO will be modified.
  // 2. If the |page| is from this VMO and offset (and was not reclaimed) then the page will have
  //    been removed from any candidate reclamation lists (such as the DontNeed pager backed list).
  // The effect of (2) is that the caller can assume in the case of reclamation failure it will not
  // keep finding this page as a reclamation candidate and infinitely retry it.
  // If the |compressor| is non-null then it must have just had |Arm| called on it.
  // |eviction_action| hints indicates whether the |always_need| eviction hint should be respected
  // or ignored. Require will force eviction.
  //
  // The actual number of pages reclaimed is returned if successful, or a failure reason if not.
  // VmCowReclaimResult is an alias for fit::result<VmCowReclaimFailure, VmCowReclaimSuccess>.
  VmCowReclaimResult ReclaimPage(vm_page_t* page, uint64_t offset, EvictionAction eviction_action,
                                 VmCompressor* compressor);

  // Helper for reclamation functions to perform common checks for whether or not reclamation should
  // proceed. It takes two parameters, one being the original requested page and the other being
  // the result of a page list Lookup or LookupMutable, allowing it to check if the page is still
  // up to date and owned by this VMO.
  template <typename T>
  ktl::optional<VmCowReclaimFailure> CannotReclaimPageLocked(vm_page_t* page, T actual)
      TA_REQ(lock());

  // If any pages in the specified range are loaned pages, replaces them with non-loaned pages
  // (which requires providing a |page_request|). The specified range should be fully committed
  // before calling this function. If a gap or a marker is encountered, or a loaned page cannot be
  // replaced, returns early with ZX_ERR_BAD_STATE. If the replacement needs to wait on the PMM for
  // allocation, returns ZX_ERR_SHOULD_WAIT, and the caller should wait on the |page_request|.
  // |non_loaned_len| is set to the length (starting at |offset|) that contains only non-loaned
  // pages. |offset| and |len| must be page-aligned.
  zx_status_t ReplacePagesWithNonLoanedLocked(VmCowRange range, DeferredOps& deferred,
                                              AnonymousPageRequest* page_request,
                                              uint64_t* non_loaned_len) TA_REQ(lock());

  // If page is still at offset, replace it with a loaned page.
  zx_status_t ReplacePageWithLoaned(vm_page_t* before_page, uint64_t offset) TA_EXCL(lock());

  // Attempts to dedup the given page at the specified offset with the zero page. The only
  // correctness requirement for this is that `page` must be *some* valid vm_page_t, meaning that
  // all race conditions are handled internally. This function returns false if
  //  * page is either not from this VMO, or not found at the specified offset
  //  * page is pinned
  //  * vmo is uncached
  //  * page is not all zeroes
  // Otherwise 'true' is returned and the page will have been returned to the pmm with a zero page
  // marker put in its place.
  bool DedupZeroPage(vm_page_t* page, uint64_t offset);

  void DumpLocked(uint depth, bool verbose) const TA_REQ(lock());

  // see VmObject::DebugLookupDepth
  uint32_t DebugLookupDepthLocked() const TA_REQ(lock());

  // Slow validation of the continuously tracked populated slots count.
  //
  // Returns true if the tracked populated slots count equals the number of pages, references, and
  // parent content markers in the local page list.
  //
  // Panics if the populated slots feature is not enabled.
  bool DebugValidateContinuousAttribution() const TA_REQ(lock());

  // Walks all the descendants in a preorder traversal. Stops if func returns anything other than
  // ZX_OK.
  zx_status_t DebugForEachDescendant(
      fit::inline_function<zx_status_t(VmCowPages* cow, uint depth)>);

  using RangeChangeOp = VmObject::RangeChangeOp;
  // Applies the specific operation to all mappings in the given range. The mappings for the current
  // object are operated on immediately, with any children being operated on using |deferred|. If
  // the caller knows that no |DeferredOps| is needed (e.g. the VMO has no children and is not pager
  // backed) then a nullptr can be provided.
  void RangeChangeUpdateLocked(VmCowRange range, RangeChangeOp op, DeferredOps* deferred)
      TA_REQ(lock());

  // Apply the specified operation to all mappings of |paged| in the given range. The provided
  // |range| must be page aligned.
  //
  // |paged| is assumed to be one of the aliases for this |VmCowPages| node. |range| must be
  // page-aligned.
  //
  // Differs from |RangeChangeUpdateLocked| in that this method does not apply the operation to
  // copy-on-write children.
  void RangeChangeUpdateMappingsLocked(VmObjectPaged& paged, VmCowRange range, RangeChangeOp op)
      TA_REQ(lock());

  // The VmObjectPaged is changing its cache mapping policy. Clean / invalidate all existing pages
  // and update page queues if required.
  void FinishCachePolicyTransitionLocked() TA_REQ(lock());

  // Promote pages in the specified range for reclamation under memory pressure. |offset| will be
  // rounded down to the page boundary, and |len| will be rounded up to the page boundary.
  // Currently used only for pager-backed VMOs to move their pages to the end of the
  // pager-backed queue, so that they can be evicted first.
  zx_status_t PromoteRangeForReclamation(VmCowRange range);

  // Protect pages in the specified range from reclamation under memory pressure. |offset| will be
  // rounded down to the page boundary, and |len| will be rounded up to the page boundary. Any
  // absent pages in the range will first be committed and then, if |set_always_need| is true, the
  // |always_need| flag in the pages will be set.
  // If the |ignore_errors| flag is set then any per page errors will be ignored and future pages in
  // the range will still be operated on. If this flag is not set then any kind of error causes an
  // immediate abort.
  zx_status_t ProtectRangeFromReclamation(VmCowRange range, bool set_always_need,
                                          bool ignore_errors);

  // Ensures any pages in the specified range are not compressed, but does not otherwise commit any
  // pages.
  zx_status_t DecompressInRange(VmCowRange range);

  zx_status_t LockRangeLocked(VmCowRange range, zx_vmo_lock_state_t* lock_state_out) TA_REQ(lock());
  zx_status_t TryLockRangeLocked(VmCowRange range) TA_REQ(lock());
  zx_status_t UnlockRangeLocked(VmCowRange range) TA_REQ(lock());

  uint64_t DebugGetPageCountLocked() const TA_REQ(lock());
  bool DebugIsPage(uint64_t offset) const;
  bool DebugIsMarker(uint64_t offset) const;
  uint32_t DebugGetMarkerShareCount(uint64_t offset) const;
  bool DebugIsParentContent(uint64_t offset) const;
  bool DebugIsEmpty(uint64_t offset) const;
  vm_page_t* DebugGetPage(uint64_t offset) const TA_EXCL(lock());
  vm_page_t* DebugGetPageLocked(uint64_t offset) const TA_REQ(lock());

  // Exposed for testing.
  DiscardableVmoTracker* DebugGetDiscardableTracker() const { return discardable_tracker_.get(); }

  bool DebugIsHighMemoryPriority() const TA_EXCL(lock());

  // See DiscardableVmoTracker::DebugDiscardablePageCounts().
  struct DiscardablePageCounts {
    uint64_t locked;
    uint64_t unlocked;
  };
  DiscardablePageCounts DebugGetDiscardablePageCounts() const TA_EXCL(lock());

  // Returns the parent of this cow pages, may be null. Generally the parent should never be
  // directly accessed externally, but this exposed specifically for tests.
  fbl::RefPtr<VmCowPages> DebugGetParent();

  // Initializes the PageCache instance for COW page allocations.
  static void InitializePageCache(uint32_t level);

  // Unlocked wrapper around ReplacePageLocked, exposed for the physical page provider to cancel
  // loans with.
  zx_status_t ReplacePage(vm_page_t* before_page, uint64_t offset, bool with_loaned,
                          vm_page_t** after_page, AnonymousPageRequest* page_request)
      TA_EXCL(lock());

  // In the given range, evict any pages that are in `page_queue`. This function should be used when
  // we want to evict for the purpose of freeing memory, and details on the specific page don't
  // matter.
  VmCowReclaimResult ReclaimRangeForEviction(uint64_t offset, size_t length,
                                             EvictionAction eviction_action);

  // Evict a specific loaned page for the use case of reclaiming loaned pages by the physical page
  // provider. Unlike ReclaimPage this function can assume it just needs to evict, and has no
  // requirements on updating any reclamation lists.
  zx_status_t EvictLoanedPage(vm_page_t* page, uint64_t offset);

  // Potentially transitions from Alive->Dead if the cow pages is unreachable (i.e. has no
  // paged_ref_ and no children). Used by the VmObjectPaged when it unlinks the paged_ref_, but
  // prior to dropping the RefPtr, giving the VmCowPages a chance to transition.
  // If a VmCowPages is returned then this is a parent that needs to have MaybeDeadTransition called
  // on it.
  fbl::RefPtr<VmCowPages> MaybeDeadTransition();

  // Helper to allocate a new page for the VMO, filling out the page request if necessary.
  zx_status_t AllocPage(vm_page_t** page, AnonymousPageRequest* page_request);

  // Helper to free |pages| to the PMM. This function will also try to invoke FreePages() on the
  // backing page source if it supports it. Given the allowance of freeing pages from any object in
  // the hierarchy, but the page source only being on the root, it is a requirement (and checked on
  // clone creation), that if a page source is handling free then it may not have CoW children.
  // There is also an equivalent assumption that if the page source is handling free, then the page
  // source will be supplying all the pages and this object must never allocate directly from the
  // PMM.
  //
  // Callers should avoid calling pmm_free() directly from inside VmCowPages, and instead should use
  // this helper.
  void FreePages(VmPageDoublyLinkedList* pages) {
    if (page_source_type() == PageSourceType::Anonymous ||
        page_source_type() == PageSourceType::UserPager) {
      CacheFree(pages, should_delay_reuse_on_free());
      return;
    }
    DEBUG_ASSERT(page_source_type() == PageSourceType::Contiguous);
    page_source_->FreePages(pages);
  }

  // Helper to free |pages| to the PMM. This function will also try to invoke FreePages() on the
  // backing page source if it supports it. Given the allowance of freeing pages from any object in
  // the hierarchy, but the page source only being on the root, it is a requirement (and checked on
  // clone creation), that if a page source is handling free then it may not have CoW children.
  // There is also an equivalent assumption that if the page source is handling free, then the page
  // source will be supplying all the pages and this object must never allocate directly from the
  // PMM.
  //
  // Callers should avoid calling pmm_free_page() directly from inside VmCowPages, and instead
  // should use this helper.
  void FreePage(vm_page_t* page) {
    DEBUG_ASSERT(!page->queue_node.InContainer());
    if (page_source_type() == PageSourceType::Anonymous ||
        page_source_type() == PageSourceType::UserPager) {
      CacheFree(page, should_delay_reuse_on_free());
      return;
    }
    DEBUG_ASSERT(page_source_type() == PageSourceType::Contiguous);
    VmPageDoublyLinkedList list;
    list.push_back(page);
    page_source_->FreePages(&list);
  }

  // Returns an appropriate PmmOptDelayReuse value for use when freeing pages from this object.
  PmmOptDelayReuse should_delay_reuse_on_free() const {
    // If this object has ever held a pinned page, we want to delay reuse of its pages to reduced
    // impact of a "bad DMA" bug should one occur.
    return ever_pinned_.load(ktl::memory_order_acquire) ? PmmOptDelayReuse::Yes
                                                        : PmmOptDelayReuse::Default;
  }

  static void DebugDumpReclaimCounters();

  // Test-only interface to get the current populated slots count.
  //
  // This method panics if the kernel is built without support for the functional continuous
  // attribution tracker (EXPERIMENTAL_CONTINUOUS_PER_VMO_ATTRIBUTION_ENABLED).
  uint32_t DebugGetPopulatedSlotsCount() const;

 private:
  // private constructor (use Create...())
  VmCowPages(VmCowPagesOptions options, uint32_t pmm_alloc_flags, uint64_t size,
             fbl::RefPtr<PageSource> page_source,
             ktl::unique_ptr<DiscardableVmoTracker> discardable_tracker, uint64_t lock_order);

  ~VmCowPages();

  friend class fbl::RefPtr<VmCowPages>;
  friend class LockedParentWalker;
  friend class PriorityChanger;
  friend void ::cpp_vm_cow_pages_free(VmCowPages*);

  DISALLOW_COPY_ASSIGN_AND_MOVE(VmCowPages);

  // Helper class for managing a locked VmCowPages referenced by a raw pointer. This helper makes it
  // easy pass around references to locked objects while retaining as much static analysis support
  // as possible.
  // This class needs to be declared fully inline here so that VmCowPages methods can reference it
  // and so that this can reference the |lock()| member of VmCowPages.
  class LockedPtr {
   public:
    LockedPtr() = default;
    ~LockedPtr() { release(); }
    LockedPtr(LockedPtr&& other) : ptr_(other.ptr_), lock_(other.take_lock()) {}
    explicit LockedPtr(VmCowPages* ptr) : LockedPtr(ptr, ptr->lock_order()) {}
    LockedPtr(VmCowPages* ptr, uint64_t lock_order) TA_EXCL(ptr->lock())
        : ptr_(ptr),
          lock_(Guard<CriticalMutex>{AssertOrderedLock, ptr->lock(), lock_order}.take()) {}
    // Take both the pointer and the lock, leaving the LockedPtr empty. Caller must take ownership
    // of the returned lock and release it.
    ktl::pair<VmCowPages*, Guard<CriticalMutex>::Adoptable> take() {
      VmCowPages* ret = ptr_;
      return {ret, take_lock()};
    }
    // Provide locked access to the underlying pointer. Must not be null.
    VmCowPages& locked() const TA_ASSERT(locked().lock()) { return *ptr_; }
    // Provide locked access toe the underlying pointer, or if the pointer is null locked access to
    // the passed in object.
    VmCowPages& locked_or(VmCowPages* self) const TA_REQ(self->lock())
        TA_ASSERT(locked_or(self).lock()) {
      if (ptr_) {
        return *ptr_;
      }
      return *self;
    }
    const VmCowPages& locked_or(const VmCowPages* self) const TA_REQ(self->lock())
        TA_ASSERT(locked_or(self).lock()) {
      if (ptr_) {
        return *ptr_;
      }
      return *self;
    }
    // Release the lock, returning the underlying pointer.
    VmCowPages* release() {
      VmCowPages* ret = ptr_;
      if (ret) {
        release_internal();
      }
      return ret;
    }

    explicit operator bool() const { return !!ptr_; }
    VmCowPages* get() const { return ptr_; }
    VmCowPages& operator*() const { return *ptr_; }
    VmCowPages* operator->() const { return ptr_; }

    LockedPtr& operator=(LockedPtr&& other) {
      release();
      auto [ptr, lock] = other.take();
      // Whatever ptr and lock are they come from a LockedPtr that is assumed to be valid, and so
      // assigning them into ourselves is assumed to valid and maintain our lock invariant.
      ptr_ = ptr;
      lock_ = ktl::move(lock);
      return *this;
    }

   private:
    // Helper for moving out the lock_ and clearing the ptr_ at the same time.
    Guard<CriticalMutex>::Adoptable&& take_lock() {
      ptr_ = nullptr;
      return ktl::move(lock_);
    }

    void release_internal() {
      Guard<CriticalMutex> guard{AdoptLock, ptr_->lock(), ktl::move(lock_)};
      ptr_ = nullptr;
    }
    // Underlying object pointer and lock. The invariant that this class maintains is that if ptr_
    // is null, then lock_ is invalid, otherwise if ptr_ is non-null then lock_ holds the adoptable
    // lock acquisition of that object.
    VmCowPages* ptr_ = nullptr;
    Guard<CriticalMutex>::Adoptable lock_;
  };

  // Helper for determining whether the current node should perform a dead transition or not.
  bool should_dead_transition_locked() const TA_REQ(lock()) {
    return !paged_ref_ && children_list_len_ == 0 && life_cycle_ == LifeCycle::Alive;
  }

  // Transitions from Alive->Dead, freeing pages and cleaning up state. Responsibility of the caller
  // to validate that it is correct to be doing this transition. If there is a parent_ then |parent|
  // is locked pointer to it and |sibling| must be as documented in |RemoveChildLocked|
  // Might return its parent_ RefPtr, which the caller must check if a dead transition is needed and
  // then release the RefPtr.
  fbl::RefPtr<VmCowPages> DeadTransitionLocked(const LockedPtr& parent, const LockedPtr& sibling)
      TA_REQ(lock());

  // A hidden VmCowPages has no associated VMO.
  bool is_hidden() const { return !!(options_ & VmCowPagesOptions::kHidden); }
  bool can_decommit_zero_pages() const {
    return !(options_ & VmCowPagesOptions::kCannotDecommitZeroPages);
  }

  PageSourceType page_source_type() const {
    if (!page_source_) {
      return PageSourceType::Anonymous;
    }
    if (page_source_->properties().is_user_pager) {
      return PageSourceType::UserPager;
    }
    return PageSourceType::Contiguous;
  }

  // Returns whether or not performing a bidirectional clone would result in a valid tree structure.
  // This does not perform checks on whether there are pinned pages, or if a bidirectional clone
  // would semantically make sense. Additionally the target |parent| for the new node should be
  // passed in, which may or may not be the same as |parent_|.
  bool can_bidirectional_clone_locked(const LockedPtr& parent) const TA_REQ(lock()) {
    // If the immediate node has a page source of any kind then bidirectional cloning is not
    // possible. A page source is otherwise permitted in the tree.
    if (page_source_) {
      return false;
    }

    // Children may not exist on the current node, as the bidirectional clone path cannot presently
    // fix them up.
    if (children_list_len_ != 0) {
      return false;
    }

    // If there is a parent then either that parent is hidden, or the parent is the root of the
    // tree. This forbids creating a bi-directional clone at the end of chain of unidirectional
    // clones.
    if (parent && parent.locked().parent_ && !parent->is_hidden()) {
      return false;
    }

    return true;
  }

  // Returns whether or not performing a unidirectional clone would result in a valid tree
  // structure. This does not mean that the a unidirectional clone would semantically make sense.
  bool can_unidirectional_clone_locked() const TA_REQ(lock()) {
    // Root must be pager-backed, otherwise we must always be doing a bidirectional clone.
    if (!is_root_source_user_pager_backed()) {
      return false;
    }

    // Any parent must not be hidden. This transitively ensures that there is a never a
    // unidirectional clone anywhere below a hidden parent.
    if (parent_ && is_parent_hidden_locked()) {
      return false;
    }

    return true;
  }

  // See |ForEveryOwnedHierarchyPageInRange|. Each entry given to `T` is constant and may not be
  // modified in any way.
  template <typename T>
  zx_status_t ForEveryOwnedHierarchyPageInRangeLocked(T func, uint64_t offset, uint64_t size,
                                                      const LockedPtr& parent) const TA_REQ(lock());

  // See |ForEveryOwnedHierarchyPageInRange|. Each entry given to `T` is a VmPageOrMarkerRef, which
  // supports limited mutation.
  template <typename T>
  zx_status_t ForEveryOwnedMutableHierarchyPageInRangeLocked(T func, uint64_t offset, uint64_t size,
                                                             const LockedPtr& parent)
      TA_REQ(lock());

  // See |ForEveryOwnedHierarchyPageInRange|. Each entry given to `T` is mutable and `T` may modify
  // it or replace it with an empty entry.
  template <typename T>
  zx_status_t RemoveOwnedHierarchyPagesInRangeLocked(T func, uint64_t offset, uint64_t size,
                                                     const LockedPtr& parent) TA_REQ(lock());

  // Iterates a range within a visible node, invoking a callback for every `VmPageListEntry` the
  // node owns (fully or partially) in that range.
  //
  // The callback is invoked at most once per each offset within the range, as the node can own at
  // most one entry at each offset. Either:
  //  * The node directly contains the first visible entry at the offset and thus fully owns it.
  //  * A hidden parent contains the first visible entry at the offset and thus the node partially
  //    owns it.
  //  * A visible parent contains the first visible entry at the offset and thus that parent fully
  //    owns it. This method doesn't evaluate such parents and skips the offset.
  //  * There is no visible entry at the offset. This method skips the offset.
  //
  // Prefer using the non-static methods above over invoking this function directly.
  //
  // The caller provides:
  //  * `self`: Node to begin the iteration from. It must be a visible node.
  //  * `func`: Callback function invoked for each non-empty entry.
  //  * `offset`: Offset relative to `self` to begin iterating at.
  //  * `size`: Size of the range to iterate.
  //  * `parent`: If the caller has locked the immediate parent, then it can pass it in here to
  //              avoid double locking, otherwise if no parent or not locked a nullptr can be given.
  //
  // The type `S` must be implicitly convertible to a `VmCowPages` or a `const VmCowPages`.
  // The type `P` is `const VmPageOrMarker` if `S` is const, otherwise it is `VmPageOrMarker`.
  // The type `T` must be:
  // `zx_status_t(P* p, const VmCowPages* owner, uint64_t self_offset, uint64_t owner_offset)`
  //  The return value controls whether iteration continues:
  //   * `ZX_ERR_NEXT`: Continue iteration or stop with `ZX_OK` if no more entries to iterate.
  //   * `ZX_ERR_STOP`: Stop iteration immediately with `ZX_OK`.
  //   * Any other code: Stop iteration immediately with that error code.
  //
  // Returns `ZX_OK` if no `func` invocations returned any errors, otherwise returns the error code
  // from the failing `func` invocation.
  template <typename P, typename S, typename T>
  static zx_status_t ForEveryOwnedHierarchyPageInRange(S* self, T func, uint64_t offset,
                                                       uint64_t size, const LockedPtr& parent)
      TA_REQ(self->lock());

  // Changes a Reference in the provided VmPageOrMarker into a real vm_page_t. The allocated page
  // is assumed to be for this VmCowPages, and so uses the pmm_alloc_flags_, but it is not assumed
  // that the page_or_mark is actually yet in this page_list_, and so the allocated page is not
  // added to the page queues. It is the responsibility of the caller to add to the page queues if
  // the page_or_mark is not stack owned.
  // The |page_request| must be non-null if the |pmm_alloc_flags_| allow for delayed allocation, in
  // which case this may return ZX_ERR_SHOULD_WAIT if the page_request is filled out.
  zx_status_t MakePageFromReference(VmPageOrMarkerRef page_or_mark,
                                    AnonymousPageRequest* page_request);

  // Replaces the Reference in VmPageOrMarker owned by this page_list_ for a real vm_page_t.
  // Unlike MakePageFromReference this updates the page queues to track the newly added page. Use
  // of |page_request| and implications on return value are the same as |MakePageFromReference|.
  zx_status_t ReplaceReferenceWithPageLocked(VmPageOrMarkerRef page_or_mark, uint64_t offset,
                                             AnonymousPageRequest* page_request) TA_REQ(lock());

  zx_status_t AllocateCopyPage(paddr_t parent_paddr, VmPageDoublyLinkedList* alloc_list,
                               AnonymousPageRequest* request, vm_page_t** clone);

  static zx_status_t CacheAllocPage(uint alloc_flags, vm_page_t** p);
  static void CacheFree(VmPageDoublyLinkedList* list, PmmOptDelayReuse delay_reuse);
  static void CacheFree(vm_page_t* p, PmmOptDelayReuse delay_reuse);

  // Helper for allocating and initializing a loaned page.
  template <typename F>
  zx::result<vm_page_t*> AllocLoanedPage(F allocated);

  // Helper for allocating a page for this VMO. The allocated page is not yet initialized, and
  // InitializeVmPage must be called on it prior to use. Callers most likely want AllocPage instead,
  // with this method being useful in rare cases where you want to defer initialization till
  // AddNewPagesLocked or similar.
  zx_status_t AllocUninitializedPage(vm_page_t** page, AnonymousPageRequest* page_request);

  // Helper for removing a page from the PageQueues and adding to a deferred ops for later freeing.
  void RemovePageLocked(vm_page_t* page, DeferredOps& ops) TA_REQ(lock());

  // Helper class for managing a two part add page transaction. This object allows adding a page to
  // be split into a check and allocation, which can fail, with the final insertion, which cannot
  // fail.
  class AddPageTransaction {
   public:
    AddPageTransaction(VmPageOrMarkerRef slot, uint64_t offset, CanOverwriteSlot overwrite)
        : slot_(slot), offset_(offset), overwrite_(overwrite) {}
    AddPageTransaction(AddPageTransaction&& other)
        : slot_(other.slot_), offset_(other.offset_), overwrite_(other.overwrite_) {
      other.slot_ = VmPageOrMarkerRef();
    }
    ~AddPageTransaction() { DEBUG_ASSERT(!slot_); }
    AddPageTransaction(const AddPageTransaction&) = delete;
    AddPageTransaction& operator=(const AddPageTransaction&) = delete;
    AddPageTransaction& operator=(AddPageTransaction&&) = delete;

    void Cancel(VmPageList& pl);
    VmPageOrMarker Complete(VmPageOrMarker p);
    uint64_t offset() const { return offset_; }
    CanOverwriteSlot overwrite() const { return overwrite_; }

   private:
    VmPageOrMarkerRef slot_;
    const uint64_t offset_;
    const CanOverwriteSlot overwrite_;
  };

  // Performs initial checks and slot allocations for inserting a new page at the specified
  // |offset|.
  //
  // |overwrite| controls how the function handles pre-existing content at |offset|. If |overwrite|
  // does not permit replacing the content, ZX_ERR_ALREADY_EXISTS will be returned.
  //
  // On success the returned |AddPageTransaction| *must* be used in a call to either
  // |CompleteAddPageLocked|, |CompleteAddNewPageLocked| or |CancelAddPageLocked|.
  //
  // No other |page_list_| operations should be performed until the transaction is complete, and
  // starting or ending a transaction should be assumed to invalidate any page_list_ iterators or
  // slots that have been looked up unless the caller explicitly knows otherwise.
  [[nodiscard]] zx::result<AddPageTransaction> BeginAddPageLocked(uint64_t offset,
                                                                  CanOverwriteSlot overwrite)
      TA_REQ(lock());

  // Similar to |BeginAddPageLocked| except only the |overwrite| checks are performed and a ready to
  // use slot is provided by the caller. It is a requirement by the caller to ensure that |slot|:
  //  * Is for |offset| in this |page_list_|
  //  * Does not fall in the middle of an interval.
  // All other requirements of |BeginAddPageLocked| otherwise apply.
  [[nodiscard]] zx::result<AddPageTransaction> BeginAddPageWithSlotLocked(
      uint64_t offset, VmPageOrMarkerRef slot, CanOverwriteSlot overwrite) TA_REQ(lock());

  // Completes an add page transaction that had been started by inserting the provided page |p| into
  // the slot looked up in |transaction|. Once complete the transaction must not be used in any
  // other complete or cancel calls.
  //
  // |p| must not be Empty()
  //
  // This operation unmaps the corresponding offset from any existing mappings, unless |deferred| is
  // a |nullptr|, in which case it will skip updating mappings.
  //
  // Any previous content in the slot is returned and must be dealt with by the caller.
  [[nodiscard]] VmPageOrMarker CompleteAddPageLocked(AddPageTransaction& transaction,
                                                     VmPageOrMarker&& p, DeferredOps* deferred)
      TA_REQ(lock());

  // Similar to |CompleteAddPageLocked| except a |vm_page_t| is provided that is assumed to not yet
  // be in the |OBJECT| state, this page may also be optionally zeroed.
  [[nodiscard]] VmPageOrMarker CompleteAddNewPageLocked(AddPageTransaction& transaction,
                                                        vm_page_t* page, bool zero,
                                                        DeferredOps* deferred) TA_REQ(lock());

  // Cancels an add page transaction and potentially frees the unused slot. It is required to call
  // this, instead of dropping an |AddPageTransaction| to ensure the page list does not gather empty
  // nodes. If the transaction was created with |BeginAddPageWithSlotLocked| then it is the
  // responsibility of the caller to know whether their slot might still be valid or not after
  // cancelling. Once cancelled the transaction must not have another further complete or cancel
  // calls against it.
  void CancelAddPageLocked(AddPageTransaction& transaction) TA_REQ(lock());

  // Helper for checking the |overwrite| conditions on a given slot.
  zx_status_t CheckOverwriteConditionsLocked(uint64_t offset, const VmPageOrMarker& slot,
                                             CanOverwriteSlot overwrite) TA_REQ(lock());

  // Add a page to the object at |offset|. This is just a wrapper around performing
  // |BeginAddPageLocked| and then |CompleteAddPageLocked|, with error handling to perform a free of
  // the page given in |p| should insertion fail. Otherwise see those methods for a description of
  // the parameters.
  zx::result<VmPageOrMarker> AddPageLocked(uint64_t offset, VmPageOrMarker&& p,
                                           CanOverwriteSlot overwrite, DeferredOps* deferred)
      TA_REQ(lock());

  // Unmaps and frees all the committed pages in the specified range.
  // Upon success the removed pages are placed in the DeferredOps freed list, and the number of such
  // pages is returned.
  //
  // Unlike DecommitRangeLocked(), this function only operates on |this| node, which must have no
  // parent.
  // |offset| must be page aligned. |len| must be less than or equal to |size_ - offset|. If |len|
  // is less than |size_ - offset| it must be page aligned.
  zx::result<uint64_t> UnmapAndFreePagesLocked(uint64_t offset, uint64_t len, DeferredOps& deferred)
      TA_REQ(lock());

  // internal check if any pages in a range are pinned
  bool AnyPagesPinnedLocked(uint64_t offset, size_t len) TA_REQ(lock());

  // Helper function for ::GetAttributedMemoryInRangeLocked. Counts the number of bytes in
  // ancestor's vmos that should be attributed to this vmo for the specified range. It is an error
  // to pass in a range that does not need attributing (i.e. offset must be < parent_limit_),
  // although |len| is permitted to be sized such that the range exceeds parent_limit_. The return
  // value is the length of the processed region, which will be <= |size| and is guaranteed to be >
  // 0. The |count| is the number of bytes in this region that should be attributed to this vmo,
  // versus some other vmo.
  uint64_t CountAttributedAncestorBytesLocked(uint64_t offset, uint64_t size,
                                              AttributionCounts* count) const TA_REQ(lock());

  // Searches for the content for the page in |this| at |offset|. If the offset is already
  // populated in |this| then that page is returned with the |owner| unset, otherwise the
  // parent hierarchy is searched for any content. The result could be used to initialize a commit,
  // or compare an existing commit with the original. The initial content is a VMPLCursor and may
  // be invalid if there was no explicit initial content. How to interpret an absence of content,
  // whether it is zero or otherwise, is left up to the caller.
  //
  // If an ancestor has a committed page which corresponds to |offset|, returns a cursor with
  // |current()| as that page as well as a LockedPtr to the VmCowPages and offset which own the
  // page. If no ancestor has a committed page for the offset, returns a cursor with a |current()|
  // of nullptr as well as the VmCowPages/offset which need to be queried to populate the page. If
  // |this| needs to be queried to populate the page, |owner| is not set, otherwise it is set to the
  // ancestor that owns the page. The reason being that |this| will already be externally locked by
  // the caller, whereas an ancestor needs to be locked inside this function.
  //
  // The returned |visible_end| represents the size of the range in the owner for which it can be
  // assumed that no child has content for, although for which the content might be in yet a higher
  // up parent. This will always be a subset of the provided |max_owner_length|, which serves as a
  // bound for the calculation and so passing in a smaller |max_owner_length| can sometimes be more
  // efficient.
  // It is an error for the |max_owner_length| to be < kPageSize.
  struct PageLookup {
    VMPLCursor cursor;
    LockedPtr owner;
    uint64_t owner_offset = 0;
    uint64_t visible_end = 0;
  };
  PageLookup FindPageContentLocked(uint64_t offset, uint64_t max_owner_length) TA_REQ(lock());

  // Searches for the initial content, i.e. the content that would be used to initially populate the
  // page, of |this| at |offset|. Whether there is presently any content populated in |this| is
  // ignored, and if there is content then this will still return what would be used to re-populate
  // that slot.
  PageLookup FindInitialPageContentLocked(uint64_t offset) TA_REQ(lock());

  // Helper function that 'forks' a page into |offset| of the current node, which must be a visible
  // node. If this function successfully inserts the page, it returns ZX_OK and populates
  // |out_page|. |page_request| must be provided and if ZX_ERR_SHOULD_WAIT is returned then this
  // indicates a transient allocation failure that should be resolved by waiting on the page_request
  // and retrying.
  //
  // The source page that is being forked has already been calculated - it is |page|, which
  // is currently in |page_owner| at offset |owner_offset|. |page_owner| must be a hidden node.
  //
  // This function is responsible for ensuring that COW clones never result in worse memory
  // consumption than simply creating a new VMO and memcpying the content. If |page| is not shared
  // at all, then this function assumes it is only accessible to this node. In that case |page| is
  // removed from |page_owner| and migrated into this node. Forking the page in that case would just
  // make |page| inaccessible, leaving |page| committed for no benefit.
  //
  // To handle memory allocation failure, this function allocates a slot in this node for the page
  // before modifying the source page or page list in |page_owner|. If that allocation fails, then
  // these are not altered.
  //
  // |page| must not be the zero-page, as there is no need to do the complex page fork logic to
  // reduce memory consumption in that case.
  zx_status_t CloneCowPageLocked(uint64_t offset, VmPageDoublyLinkedList* alloc_list,
                                 VmCowPages* page_owner, vm_page_t* page, uint64_t owner_offset,
                                 DeferredOps& deferred, AnonymousPageRequest* page_request,
                                 vm_page_t** out_page) TA_REQ(lock()) TA_REQ(page_owner->lock());

  zx_status_t ForkMarkerLocked(uint64_t offset, VmPageDoublyLinkedList* alloc_list,
                               VmCowPages* marker_owner, VmPageOrMarkerRef marker,
                               uint64_t owner_offset, DeferredOps& deferred,
                               AnonymousPageRequest* page_request, vm_page_t** out_page)
      TA_REQ(lock()) TA_REQ(marker_owner->lock());

  // Helper function for reducing the share count of content in a hidden node, and freeing it if it
  // is no longer referenced.
  //
  // This method assumes that the caller is overriding the slot in a child and that it will perform
  // any necessary range change updates etc.
  void DecrementCowContentShareCount(const VmPageOrMarker& content, uint64_t offset,
                                     ScopedPageFreedList& list, VmCompression* compression)
      TA_REQ(lock());

  // Helper struct which encapsulates a parent node along with a range and limit relative to it.
  struct ParentAndRange {
    LockedPtr parent;
    LockedPtr grandparent;
    uint64_t parent_offset;
    uint64_t parent_limit;
    uint64_t size;
  };

  // Helper function for |CreateCloneLocked|.
  //
  // Walks the hierarchy from the |this| node to the root and finds the most distant node which
  // could correctly be the parent for a new clone of this node.
  //
  // Computes a range and limit for the clone, relative to the final parent, such that the clone
  // can't see any more of that parent than it could if it was a direct clone of `this` node.
  //
  // For correctness the parent is either:
  //  * The last node encountered which satisfies `parent_must_be_hidden` if its true. The initial
  //    `parent` node satisfies `parent_must_be_hidden` even though its visible, because it may be
  //    converted to a hidden node by the caller.
  //  * The first node encountered that has pages in the clone's range for it to snapshot.
  //  * The root.
  //
  // The caller provides:
  //  * `this`: Initial candidate parent node to begin the search from. It must be a visible node.
  //             The new clone is logically a clone of it.
  //  * `offset`: Offset of the clone relative to the initial parent.
  //  * `size`: Size of the clone.
  //  * `parent_must_be_hidden`: true iff the final parent must satisfy this constraint.
  //
  // Returns the actual node that should be used as the parent of the clone, and if that node has a
  // parent also returns a locked reference to that node. By locking the parent of the target parent
  // the caller ensures that any reasoning that made the target valid remains valid until the clone
  // can be created. As a result it is only valid to create the clone if done so whilst continuously
  // holding the returned locks.
  ParentAndRange FindParentAndRangeForCloneLocked(uint64_t offset, uint64_t size,
                                                  bool parent_must_be_hidden) TA_REQ(lock());

  // Helper function for |CreateCloneLocked|.
  //
  // Performs a clone by creating a new hidden parent, under which both the clone and this node will
  // hang. The range [|offset|,|offset| + |limit|) in this node become read-only and are
  // copy-on-write with the child. Anything in the |parent| above |this| in that range are also
  // copy-on-write with the child.
  //
  // If there is a parent_ then the passed in |parent| is a locked ptr to it.
  // An |initial_page_list| may be passed in to populate the clone's page list with any parent
  // content markers if needed. In this case, |initial_page_list_tracker| must be tracking the
  // number of populated slots in |initial_page_list|.
  zx::result<LockedRefPtr> CloneNewHiddenParentLocked(
      uint64_t offset, uint64_t limit, uint64_t size, VmPageList&& initial_page_list,
      AttributionTracker&& initial_page_list_tracker, const LockedPtr& parent) TA_REQ(lock());

  // Helper function for |CreateCloneLocked|.
  //
  // Performs a clone operation by hanging a new child under |this|.
  // Unlike |CloneNewHiddenParentLocked|, items in the range [|offset|,|offset| + |limit|) do not
  // become read-only, rather they remain writable by |this| and are copy-on-write in the child.
  // Anything in the |parent| above |this| in that range are also copy-in-write with the child.
  //
  // If there is a parent_ then the passed in |parent| is a locked ptr to it.
  // An |initial_page_list| may be passed in to populate the clone's page list with any parent
  // content markers if needed. In this case, |initial_page_list_tracker| must be tracking the
  // number of populated slots in |initial_page_list|.
  zx::result<LockedRefPtr> CloneChildLocked(uint64_t offset, uint64_t limit, uint64_t size,
                                            VmPageList&& initial_page_list,
                                            AttributionTracker&& initial_page_list_tracker,
                                            const LockedPtr& parent) TA_REQ(lock());

  // Release any pages this VMO can reference from the provided start offset till the end of the
  // VMO. This releases both directly owned pages, as well as pages in hidden parents that may be
  // considered owned by this VMO.
  // If applicable this method will update the parent_limit_ to reflect that it has removed any
  // reference to its parent range, and it can be assumed that upon return that
  // |parent_limit_ <= start|.
  // The caller is responsible for actually freeing the pages, which are returned in freed_list.
  // If the caller has locked the immediate parent, then it can pass it in as |parent| to avoid
  // double locking, otherwise if no parent or not locked a nullptr can be given.
  void ReleaseOwnedPagesLocked(uint64_t start, const LockedPtr& parent,
                               ScopedPageFreedList& freed_list) TA_REQ(lock()) {
    ReleaseOwnedPagesRangeLocked(start, size_ - start, parent, freed_list);
  }

  // Similar to |ReleaseOwnedPagesLocked|, but only releases the specified range, and as such
  // provides no guarantee on the value of |parent_limit_|, with respect to |offset|, requiring the
  // caller to handle any issues with potential visibility into the parent range and content that
  // should no longer be referenced. More specifically, whereupon the conclusion of
  // |ReleaseOwnedPagesLocked| it can be assumed that |parent_limit_ <= start| it CANNOT be assumed
  // that |parent_limit_ <= offset|.
  void ReleaseOwnedPagesRangeLocked(uint64_t offset, uint64_t len, const LockedPtr& parent,
                                    ScopedPageFreedList& freed_list) TA_REQ(lock());

  // When cleaning up a hidden vmo, merges the hidden vmo's content (e.g. page list, view
  // of the parent) into the remaining child.
  // This can fail (return false) indicating content may have been partially merged. If this happens
  // the two vmos are in a valid state and can continue being used, but the parent must be retained
  // and cannot be deleted.
  bool MergeContentWithChildLocked() TA_REQ(lock());

  // Moves an existing page to the wired queue as a consequence of the page being pinned.
  void MoveToPinnedLocked(vm_page_t* page, uint64_t offset) TA_REQ(lock());

  // Updates the page queue of an existing non-pinned page, moving it to whichever queue is
  // appropriate.
  void MoveToNotPinnedLocked(vm_page_t* page, uint64_t offset) TA_REQ(lock());

  // Places a newly added, not yet pinned, page into the appropriate page queue.
  void SetNotPinnedLocked(vm_page_t* page, uint64_t offset) TA_REQ(lock());

  // Updates the page's dirty state to the one specified, and also moves the page between page
  // queues if required by the dirty state. |dirty_state| should be a valid dirty tracking state,
  // i.e. one of Clean, AwaitingClean, or Dirty.
  //
  // |offset| is the page-aligned offset of the page in this object.
  //
  // |is_pending_add| indicates whether this page is yet to be added to this object's page list,
  // false by default. If the page is yet to be added, this function will skip updating the page
  // queue as an optimization, since the page queue will be updated later when the page gets added
  // to the page list. |is_pending_add| also helps determine certain validation checks that can be
  // performed on the page.
  void UpdateDirtyStateLocked(vm_page_t* page, uint64_t offset, DirtyState dirty_state,
                              bool is_pending_add = false) TA_REQ(lock());

  // Helper to invalidate any DIRTY requests in the specified range by spuriously resolving them.
  void InvalidateDirtyRequestsLocked(uint64_t offset, uint64_t len) TA_REQ(lock());

  // Helper to invalidate any READ requests in the specified range by spuriously resolving them.
  void InvalidateReadRequestsLocked(uint64_t offset, uint64_t len) TA_REQ(lock());

  // Removes the specified child from this objects |children_list_| and performs any hierarchy
  // updates that need to happen as a result. This does not modify the |parent_| member of the
  // removed child and if this is not being called due to |removed| being destructed it is the
  // callers responsibility to correct parent_.
  // If |removed| has a sibling to its right (i.e. next in the children_list_) then |sibling| must
  // be a locked pointer to it. The exception being if this is a hidden node with two children, in
  // which case if |removed| is the right child then |sibling| should be set to the left child.
  void RemoveChildLocked(VmCowPages* removed, const LockedPtr& sibling) TA_REQ(lock())
      TA_REQ(removed->lock());

  // Inserts a newly created VmCowPages into this hierarchy as a child of this VmCowPages.
  // Initializes child members based on the passed in values that only have meaning when an object
  // is a child. This updates the parent_ field in child to hold a refptr to |this|.
  void AddChildLocked(VmCowPages* child, uint64_t offset, uint64_t parent_limit) TA_REQ(lock())
      TA_REQ(child->lock());

  void ReplaceChildLocked(VmCowPages* old, VmCowPages* new_child) TA_REQ(lock());

  void DropChildLocked(VmCowPages* c) TA_REQ(lock());

  // Helper to check whether the requested range for LockRangeLocked() / TryLockRangeLocked() /
  // UnlockRangeLocked() is valid.
  bool IsLockRangeValidLocked(VmCowRange range) const TA_REQ(lock());

  // If page is still at offset, replace it with a different page.  If with_loaned is true, replace
  // with a loaned page.  If with_loaned is false, replace with a non-loaned page and a page_request
  // is required to be provided.
  zx_status_t ReplacePageLocked(vm_page_t* before_page, uint64_t offset, bool with_loaned,
                                vm_page_t** after_page, DeferredOps& deferred,
                                AnonymousPageRequest* page_request) TA_REQ(lock());

  // Copies the metadata information (dirty state, split bit information etc) from src_page to
  // dst_page in preparation for replacing src with dst. This copies the metadata information only
  // and not the contents, that must be done using the |CopyPageContentsForReplacementLocked|
  // method. This is split into two steps to allow the copying of the metadata and installation into
  // the page queues to be done under the pmm loaned pages lock, and then the copying of the page
  // contents to be done after with the loaned pages lock dropped.
  void CopyPageMetadataForReplacementLocked(vm_page_t* dst_page, vm_page_t* src_page)
      TA_REQ(lock());

  // Copies the page contents from src_page->dst_page to complete the replacement process. Typical
  // usage would have |CopyPageMetadatForReplacementLocked| already be performed, but this method
  // does not require it.
  void CopyPageContentsForReplacementLocked(vm_page_t* dst_page, vm_page_t* src_page)
      TA_REQ(lock());

  // Internal helper for performing reclamation via compression on an anonymous VMO. Assumes that
  // the provided |compressor| is not-null.
  VmCowReclaimResult ReclaimPageForCompression(vm_page_t* page, uint64_t offset,
                                               VmCompressor* compressor);

  // Internal helper for performing reclamation against a discardable VMO. If any discarding happens
  // the number of pages is returned. The passed in |page| must be the first page in the discardable
  // VMO to trigger a discard, otherwise it will fail.
  zx::result<uint64_t> ReclaimDiscardable(vm_page_t* page, uint64_t offset);

  // Internal helper for discarding a VMO. Will discard if VMO is unlocked returning the count.
  zx::result<uint64_t> DiscardPagesLocked(DeferredOps& deferred) TA_REQ(lock());

  // Specialized internal version of ZeroPagesLocked that only operates for
  // pager-backed VMOs backed by non-user pagers.
  void ZeroPagesContiguous(VmCowRange range) TA_REQ(lock());

  // Specialized internal version of ZeroPagesLocked that only operates for a
  // VMO where there is no direct page_source_.
  ktl::pair<zx_status_t, uint64_t> ZeroPagesNoDirectPageSourceLocked(VmCowRange range,
                                                                     DeferredOps& deferred,
                                                                     MultiPageRequest* page_request)
      TA_REQ(lock());

  // Specialized internal version of ZeroPagesLocked that only operates for a VMO with a user pager.
  // |dirty_track| can be set to |true| if any zeroes inserted are to be treated as Dirty, otherwise
  // they are not dirty tracked.
  //
  // Returns the number of bytes that were actually zeroed, which may be
  // nonzero even if the returned status != ZX_OK.
  ktl::pair<zx_status_t, uint64_t> ZeroPagesDirectUserPagerLocked(VmCowRange range,
                                                                  bool dirty_track,
                                                                  DeferredOps& deferred,
                                                                  MultiPageRequest* page_request)
      TA_REQ(lock());

  // Applies the specific operation to all mappings in the given range against descendants/cow
  // children. The operation is not applied for this object. Only the DeferredOps is expected to
  // call this.
  // Takes ownership, and will drop, the lock for this object as children are iterated.
  static void RangeChangeUpdateCowChildren(LockedPtr self, VmCowRange range, RangeChangeOp op);

  // magic value
  fbl::Canary<fbl::magic("VMCP")> canary_;

  const uint32_t pmm_alloc_flags_;

  const VmCowPagesOptions options_;

  // length of children_list_
  uint32_t children_list_len_ TA_GUARDED(lock()) = 0;

  mutable LOCK_DEP_INSTRUMENT(VmCowPages, CriticalMutex, lockdep::LockFlagsNestable) lock_;

  // When acquiring multiple locks they must be acquired in order from lowest to highest. To support
  // unidirectional clones, where nodes gain new children, and bidirectional clones, where nodes
  // gain new parents, lock ordering is determined using the following scheme:
  //  * A node with a page source, as it will always be the root, is given the highest order of
  //    kLockOrderRoot.
  //  * The first anonymous node in a chain is given the a lock order in the middle of
  //    kLockOrderFirstAnon. This is nodes such as:
  //    - Direct child of a root page source node.
  //    - Direct Child of a hidden node.
  //    - New anonymous root node.
  //  * Children of visible anonymous nodes, i.e. unidirectional clones of a non-hidden non pager
  //    backed node, take their parents lock order minus the kLockOrderDelta.
  //  * Hidden nodes take either kLockOrderRoot, if they are becoming the root node, or their
  //    parents lock order minus the kLockOrderDelta.
  // The goal of this scheme is to provide room in the numbering for both unidirectional children
  // to grow down at the bottom, and hidden nodes to grow down in the middle, without colliding. If
  // children of hidden nodes did not start at kLockOrderFirstAnon, but instead just took a minimum
  // lock order, then a collision would occur if:
  //  1. A pager backed node is created that then has a hidden node below it, with two anonymous
  //     leaf nodes below it.
  //  2. A new clone is created from one of those leafs that can hang directly off the hidden node.
  //  3. Both the original leaf nodes are closed, merging the remaining child with the hidden node.
  //  4. A unidirectional clone is now created from what is now a unidirectional hierarchy.
  // Here, space is needed to grow down, as we have effectively found a way to promote a leaf child
  // of a hidden node to being part of a unidirectional clone chain.
  //
  // Having a non-contiguous numbering allows for using an alternate lock ordering scheme during
  // clone construction and dead transitions. When creating new nodes since there are no other
  // references the lock cannot be held and so we cannot deadlock. However we still need to provide
  // a lock order to satisfy lockdep. Here the gaps created by kLockOrderDelta can be used as the
  // order for these newly created nodes.
  //
  // During a dead transition we potentially need to hold locks of three nodes: the parent node and
  // two of its children. Here the order is that the children must be acquired in list order, and
  // then the parent. When acquiring the second child, since its lock order would be equal to the
  // first child, the guaranteed gap between the first child and the parent lock order is used
  // instead.
  static constexpr uint64_t kLockOrderDelta = 3;
  static constexpr uint64_t kLockOrderRoot = UINT64_MAX - kLockOrderDelta;
  static constexpr uint64_t kLockOrderFirstAnon = UINT64_MAX / 2;
  // As lock orders are only validated when lockdep is enabled, the storage is only defined if
  // lockdep is enabled.
#if (LOCK_DEP_ENABLED_FEATURE_LEVEL > 0)
  const uint64_t lock_order_;
#endif

  uint64_t size_ TA_GUARDED(lock());
  // Offset in the *parent* where this object starts.
  uint64_t parent_offset_ TA_GUARDED(lock()) = 0;
  // Offset in *this object* above which accesses will no longer access the parent.
  uint64_t parent_limit_ TA_GUARDED(lock()) = 0;
  // Offset in our root parent where this object would start if projected onto it. This value is
  // used as an efficient summation of accumulated offsets to ensure that an offset projected all
  // the way to the root would not overflow a 64-bit integer. Although actual page resolution
  // would never reach the root in such a case, a childs full range projected onto its parent is
  // used to simplify some operations and so this invariant of not overflowing accumulated offsets
  // needs to be maintained.
  uint64_t root_parent_offset_ TA_GUARDED(lock()) = 0;

  // parent pointer (may be null)
  fbl::RefPtr<VmCowPages> parent_ TA_GUARDED(lock());

  // list of every child
  fbl::TaggedDoublyLinkedList<VmCowPages*, internal::ChildListTag> children_list_
      TA_GUARDED(lock());

  // To support iterating over a subtree a cursor object is used and installed in nodes as they are
  // iterated. This ensures that if iteration races with any node destruction that the cursor can be
  // used to perform fixups.
  // Any cursors in these lists are processed (i.e. moved) during a dead transition, and so it is
  // invalid to perform an iteration over a non-alive node / subtree. Equivalently the cursor itself
  // relies on this fact to allow it to safely store raw pointer backlinks, knowing they will always
  // be cleared in a dead transition prior to the pointer becoming invalid.
  // Both the root (i.e. start and final termination point) and the current location of any cursor
  // needs to be tracked, as these both need potential updates.
  struct RootListTag {};
  struct CurListTag {};
  class TreeWalkCursor;
  fbl::TaggedDoublyLinkedList<TreeWalkCursor*, RootListTag> root_cursor_list_ TA_GUARDED(lock());
  fbl::TaggedDoublyLinkedList<TreeWalkCursor*, CurListTag> cur_cursor_list_ TA_GUARDED(lock());

  // Counts the total number of pages pinned by ::CommitRange. If one page is pinned n times, it
  // contributes n to this count.
  uint64_t pinned_page_count_ TA_GUARDED(lock()) = 0;

  // The page source, if any.
  const fbl::RefPtr<PageSource> page_source_;

  // Count reclamation events so that we can report them to the user.
  uint64_t reclamation_event_count_ TA_GUARDED(lock()) = 0;

  // a tree of pages
  VmPageList page_list_ TA_GUARDED(lock());

  // Tracks the populated bytes count and its high-water mark.
  //
  // Mark it as __NO_UNIQUE_ADDRESS so that we don't pessimize the
  // case when AttributionTracker may be empty.
  __NO_UNIQUE_ADDRESS AttributionTracker continuous_attribution_tracker_ TA_GUARDED(lock());

  // Reference back to a VmObjectPaged, which should be valid at all times after creation until the
  // VmObjectPaged has been destroyed, unless this is a hidden node. We use this in places where we
  // have access to the VmCowPages and need to look up the "owning" VmObjectPaged for some
  // information, e.g. when deduping zero pages, for performing cache or mapping updates, for
  // inserting references to the reference list.
  //
  // This is a raw pointer to avoid circular references, the VmObjectPaged destructor needs to
  // update it.
  VmObjectPaged* paged_ref_ TA_GUARDED(lock()) = nullptr;

  // Non-null if this is a discardable VMO.
  const ktl::unique_ptr<DiscardableVmoTracker> discardable_tracker_;

  // Count of how many references to this VMO are requesting this be high priority, where references
  // include VmMappings and children. If this is >0 then it is considered high priority and any kind
  // of reclamation will be disabled. Further, if this is >0 and this has a parent, then this will
  // contribute a +1 count towards its parent.
  //
  // Due to the life cycle of a VmCowPages it is expected that at the point this is destroyed it has
  // a count of 0. This is because that to be destroyed we must have no mappings and no children,
  // i.e. no references, and so nothing can be contributing to a positive count.
  //
  // It is an error for this value to ever become negative.
  int64_t high_priority_count_ TA_GUARDED(lock()) = 0;

  // This is a one-way bool (false -> true) indicating whether this object has ever held pinned
  // pages.
  //
  // With this bool we achieve these things:
  //  * Avoid using loaned pages for a VMO that will just get pinned and replace the loaned pages
  //    with non-loaned pages again, possibly repeatedly.
  //  * Avoid increasing pin latency in the (more) common case of pinning a VMO the 2nd or
  //    subsequent times (vs the 1st time).
  //  * Once we have any form of active sweeping (of data from non-loaned to loaned physical pages)
  //    this bool is part of mitigating any potential DMA-while-not-pinned (which is not permitted
  //    but is also difficult to detect or prevent without an IOMMU).
  //
  // This value is an atomic because it can be read without holding the lock.
  //
  // The rules for accessing this value are as follows:
  //  * Writes must have release semantics and be performed while holding |lock()|.
  //  * For reads performed while holding |lock()|, relaxed semantics are sufficient.
  //  * All other reads must use acquire semantics.
  ktl::atomic<bool> ever_pinned_ = false;

  // Tracks whether this VMO was modified (written / resized) if backed by a pager. This gets reset
  // to false if QueryPagerVmoStatsLocked() is called with |reset| set to true.
  bool pager_stats_modified_ TA_GUARDED(lock()) = false;

  // Tracks the life cycle of the VmCowPages. The primary purpose of the life cycle is to create an
  // invariant that by the time a VmCowPages destructor runs it does not contain any pages. This is
  // achieved by requiring an explicit Dead transition that provides a point to perform cleanup.
  // An Init state is introduced to allow for multi step creation that may fail.
  enum class LifeCycle : uint8_t {
    Init,
    Alive,
    Dying,
    Dead,
  };
  LifeCycle life_cycle_ TA_GUARDED(lock()) = LifeCycle::Init;

  // PageCache instance for COW page allocations.
  static page_cache::PageCache page_cache_;
};

// Implements a cursor that allows for retrieving successive pages over a range in a VMO. The
// range that is iterated is determined at construction from GetLookupCursorLocked and cannot be
// modified, although it can be effectively shrunk by ceasing queries early.
//
// The cursor is designed under the assumption that the caller is tracking, implicitly or
// explicitly, how many queries have been done, and the methods do not return errors if more slots
// are queried than was originally requested in the range. They will, however, assert and panic.
//
// There are three controls provided by this object.
//
//   Zero forks: By default new zero pages will be considered zero forks and added to the zero page
//   scanner list, this can be disabled with |DisableZeroFork|.
//
//   Access time: By default pages that are returned will be considered accessed. This can be
//   changed with |DisableMarkAccessed|.
//
//   Allocation lists: By default pages will be acquired from the pmm as needed. An allocation list
//   can be given use |GiveAllocList|.
//
// The VMO lock *must* be held contiguously from the call to GetLookupCursorLocked over the entire
// usage of this object. Callers can assume that for any vm_page_t* that is returned that the lock
// of the owner of that page is held up until the next operation performed on the cursor.
class VmCowPages::LookupCursor {
 public:
  ~LookupCursor() {
    InvalidateCursor();
    DEBUG_ASSERT(!alloc_list_);
  }

  // Convenience struct holding the return result of the Require* methods.
  struct RequireResult {
    vm_page_t* page = nullptr;
    bool writable = false;
  };

  // The Require* methods will attempt to lookup the next offset in the VMO and return you a page
  // with the properties requested. If a page can be returned in the zx::ok result then the internal
  // cursor is incremented and future operations will act on the next offset. If an error occurs
  // then the internal cursor is not incremented.
  // These methods all take a PageRequest, which will be populated in the case of returning
  // ZX_ERR_SHOULD_WAIT. For optimal page request generation the |max_request_pages| controls how
  // many pages you are intending to lookup, and |max_request_pages| must not exceed the remaining
  // window of the cursor.
  // The returned page, unless it was just allocated, will have its access time updated based on
  // |EnableMarkAccessed|, with newly allocated pages always being default considered to have just
  // been accessed.

  // Returned page must be an allocated and owned page in this VMO. As such this will never return a
  // reference to the zero page. |will_write| indicates if this page needs to be writable or not,
  // which for an owned and allocated page just involves a potential dirty request / transition.
  zx::result<RequireResult> RequireOwnedPage(bool will_write, uint64_t max_request_pages,
                                             DeferredOps& deferred, MultiPageRequest* page_request)
      TA_REQ(lock());

  // Returned page will only be read from. This can return zero pages or pages from a parent VMO.
  // A DeferredOps is required to be passed in, even though a Read does not ever directly generate
  // any deferred actions, to enforce the requirement that all operations on a pager backed VMO are
  // serialized with the paged_vmo_lock. Having to present a DeferredOps here is a simple way to
  // ensure this lock is held.
  zx::result<RequireResult> RequireReadPage(uint64_t max_request_pages, DeferredOps& deferred,
                                            MultiPageRequest* page_request) TA_REQ(lock());

  // Returned page will be readable or writable based on the |will_write| flag.
  zx::result<RequireResult> RequirePage(bool will_write, uint64_t max_request_pages,
                                        DeferredOps& deferred, MultiPageRequest* page_request)
      TA_REQ(lock()) {
    // Being writable implies owning the page, so forward to the correct operation.
    if (will_write) {
      return RequireOwnedPage(true, max_request_pages, deferred, page_request);
    }
    return RequireReadPage(max_request_pages, deferred, page_request);
  }

  // The IfExistPages methods is intended to be cheaper than the Require* methods and to allow for
  // performing actions if pages already exist, without performing allocations. As a result this
  // may fail to return pages in scenarios that Require* methods would, and in general are allowed
  // to always fail for any reason.
  // These methods cannot generate page requests and will not perform allocations or otherwise
  // mutate the VMO contents and will not update the access time of the pages.

  // Walks up to |max_pages| from the current offset, filling in |paddrs| as long as there are
  // actual pages and, if |will_write| is true, that they can be written to. The return value is
  // the number of contiguous pages found and filled into |paddrs|, and the cursor is incremented
  // by that many pages.
  uint64_t IfExistPages(bool will_write, uint64_t max_pages, paddr_t* paddrs) TA_REQ(lock());

  // Checks the current slot for a page and returns it. This does not return zero pages and, due to
  // the lack of taking a page request, will not perform copy-on-write allocations or dirty
  // transitions. In these cases it will return nullptr even though there is content.
  // The internal cursor is always incremented regardless of the return value.
  vm_page_t* MaybePage(bool will_write) TA_REQ(lock());

  // Has similar properties of |MaybePage|, except it returns how many times in a row |MaybePage|
  // would have returned a nullptr. Regardless of the return value of this method, it is not
  // guaranteed that the next call to |MaybePage| will not be a nullptr. The cursor is incremented
  // by the number of pages returned.
  uint64_t SkipMissingPages() TA_REQ(lock());

  // Provides a list of pages that can be used to service any allocations. This is useful if you
  // know you will be looking up multiple absent pages and want to avoid repeatedly hitting the pmm
  // for single pages.
  // If a list is provided then ClearAllocList must be called prior to the cursor being destroyed.
  void GiveAllocList(VmPageDoublyLinkedList* alloc_list) {
    DEBUG_ASSERT(alloc_list);
    alloc_list_ = alloc_list;
  }

  // Clears any remaining allocation list. This does not free any remaining pages, and it is the
  // callers responsibility to check the list and free any pages.
  void ClearAllocList() {
    DEBUG_ASSERT(alloc_list_);
    alloc_list_ = nullptr;
  }

  // Disables placing newly allocated zero pages in the zero fork list.
  void DisableZeroFork() { zero_fork_ = false; }

  // Indicates that any existing pages that are returned should not be considered accessed and have
  // their accessed times updated.
  void DisableMarkAccessed() { mark_accessed_ = false; }

  // Exposed for lock assertions.
  Lock<CriticalMutex>* lock() const TA_RET_CAP(target_->lock_ref()) { return target_->lock(); }
  Lock<CriticalMutex>& lock_ref() const TA_RET_CAP(target_->lock_ref()) {
    return target_->lock_ref();
  }

  LookupCursor(LookupCursor& other) = delete;
  LookupCursor(LookupCursor&& other) = default;

  LookupCursor(VmCowPages* target, VmCowRange range)
      : target_(target),
        offset_(range.offset),
        end_offset_(range.end()),
        target_directly_backed_by_user_pager_(target->page_source_type() ==
                                              PageSourceType::UserPager),
        zero_fork_(!target_directly_backed_by_user_pager_ && target->can_decommit_zero_pages()) {}

 private:
  // Note: Some of these methods are marked __ALWAYS_INLINE as doing so has a dramatic performance
  // improvement, and is worth the increase in code size. Due to gcc limitations to mark them
  // __ALWAYS_INLINE they need to be declared here in the header.

  // Increments the cursor to the next offset. Doing so may invalidate the cursor and requiring
  // recalculating.
  __ALWAYS_INLINE void IncrementCursor() TA_REQ(lock()) {
    offset_ += kPageSize;
    if (offset_ == owner_info_.visible_end) {
      // Have reached either the end of the valid iteration range, or the end of the visible portion
      // of the owner. In the latter case we invalidate the cursor as we need to walk up the
      // hierarchy again to find the next owner that applies to this slot.
      // In the case where we have reached the end of the range, i.e. offset_ is also equal to
      // end_offset_, there is nothing we need to do, but to ensure that an error is generated if
      // the user incorrectly attempts to get another page we also invalidate the owner.
      InvalidateCursor();
    } else {
      // Increment the owner offset and step the page list cursor to the next slot.
      owner_info_.owner_offset += kPageSize;
      owner_info_.cursor.step();
      owner_cursor_ = owner_info_.cursor.current_ref();

      // When iterating, it's possible that we need to find a new owner even before we hit the
      // visible_end. This happens since even if we have no content at our cursor, we might have a
      // parent with content, and the visible_end is tracking the range visible in us from the
      // target and does not imply we have all the content.
      // Consider a simple hierarchy where the root has a page in slot 1, [.P.], then its child has
      // a page in slot 0 [P...] and then its child, the target, has no pages [...] A cursor on this
      // range will initially find the owner as this middle object, and a visible length of 3 pages.
      // However, when we step the cursor we clearly need to then walk up to our parent to get the
      // page. In this case we would ideally walk up to the parent, if there is one, and check for
      // content, or if no parent keep returning empty slots. Unfortunately once the cursor returns
      // a nullptr we cannot know where the next content might be. To make things simpler we just
      // invalidate owner if we hit this case and re-walk from the bottom again.
      // Whether or not a parent might have content is a combination of
      //  1. There must be a parent and the offset within the parent limit
      //  2. Either the slot is empty, meaning we see the parent, and the node does not use parent
      //     content markers. Or there is a parent content marker.
      auto can_see_parent = [&]() TA_REQ(lock()) -> bool {
        if (!owner_info_.owner.locked_or(target_).parent_) {
          return false;
        }
        if (owner_info_.owner_offset >= owner_info_.owner.locked_or(target_).parent_limit_) {
          return false;
        }
        if (owner_info_.owner.locked_or(target_).node_has_parent_content_markers()) {
          return owner_cursor_->IsParentContent();
        }
        return owner_cursor_->IsEmpty();
      };
      if (!owner_cursor_ || can_see_parent()) {
        InvalidateCursor();
      }
    }
  }

  // Increments the current offset by the given delta, but invalidates the cursor itself requiring
  // it to be recalculated next time EstablishCursor is called.
  void IncrementOffsetAndInvalidateCursor(uint64_t delta);

  // Returns whether the cursor is currently valid or needs to be re-calculated.
  bool IsCursorValid() const { return is_valid_; }

  // Calculates the current cursor, finding the correct owner, owner offset etc. There is always an
  // owner and this process can never fail.
  void EstablishCursor() TA_REQ(lock());

  // Returns true if target_ is the owner.
  bool TargetIsOwner() const { return !owner_info_.owner; }

  // Invalidates the owner, so that the next page will have to perform the lookup again, walking up
  // the hierarchy if needed.
  void InvalidateCursor() {
    // Mark the cursor as invalid, but do not drop the owner lock. This ensures that the up until
    // the next page lookup is performed the lock for the owner of the page most recently returned
    // remains held.
    is_valid_ = false;
  }

  // Helpers for querying the state of the cursor.
  bool CursorIsPage() const { return owner_cursor_ && owner_cursor_->IsPage(); }
  bool CursorIsMarker() const { return owner_cursor_ && owner_cursor_->IsMarker(); }
  bool CursorIsEmpty() const { return !owner_cursor_ || owner_cursor_->IsEmpty(); }
  bool CursorIsParentContent() const { return owner_cursor_ && owner_cursor_->IsParentContent(); }
  bool CursorIsReference() const { return owner_cursor_ && owner_cursor_->IsReference(); }
  // Checks if the cursor is exactly at a sentinel, and not generally inside an interval.
  bool CursorIsIntervalZero() const { return owner_cursor_ && owner_cursor_->IsIntervalZero(); }

  // Checks if the cursor, as determined by the current offset and not the literal
  // owner_info_.cursor, is in a zero interval.
  bool CursorIsInIntervalZero() const TA_REQ(lock()) {
    return CursorIsIntervalZero() ||
           owner_info_.owner.locked_or(target_).page_list_.IsOffsetInZeroInterval(
               owner_info_.owner_offset);
  }

  // The cursor can be considered to have content of zero if either it points at a zero marker, or
  // the cursor itself is empty and content is initially zero. Content is initially zero if either
  // there isn't a page source, or the offset is in a zero interval.
  // If a page source is not a user pager then we could consider it to be zero, except we
  // would not necessarily be able to fork that zero page to create an owned/writable page. In
  // practice this case only exists for contiguous VMOs, and the way they are used makes optimizing
  // to return the zero page in the case of reads not beneficial.
  bool CursorIsContentZero() const TA_REQ(lock());

  // A usable page is either just any page, if not writing, or if writing, a page that is owned by
  // the target and doesn't need any dirty transitions. i.e., a page that is ready to use right now.
  bool CursorIsUsablePage(bool writing) {
    return CursorIsPage() && (!writing || (TargetIsOwner() && !TargetDirtyTracked()));
  }

  // Determines whether the zero content at the current cursor should be supplied as dirty or not.
  // This is only allowed to be called if CursorIsContentZero is true.
  bool TargetZeroContentSupplyDirty(bool writing) const TA_REQ(lock());

  // Returns whether the target is tracking the dirtying of content with dirty pages and dirty
  // transitions.
  bool TargetDirtyTracked() const { return target_directly_backed_by_user_pager_; }

  // Turns the supplied page into a result. Does not increment the cursor. |in_target| specifies
  // whether the page is known to be in target_ or in some parent object.
  RequireResult PageAsResultNoIncrement(vm_page_t* page, bool in_target);

  // Turns the current cursor, which must be a page, into a result and handles any access time
  // updating. Increments the cursor.
  __ALWAYS_INLINE RequireResult CursorAsResult() TA_REQ(lock()) {
    if (mark_accessed_) {
      Pmm::Node().GetPageQueues()->MarkAccessed(owner_cursor_->Page());
    }
    // Inform PageAsResult whether the owner is the target_, but otherwise let it calculate the
    // actual writability of the page.
    RequireResult result = PageAsResultNoIncrement(owner_cursor_->Page(), TargetIsOwner());
    IncrementCursor();
    return result;
  }

  // Allocates a new page for the target that is a copy of the provided |source| page. On success
  // page is inserted into target at the current offset_ and the cursor is incremented.
  zx::result<RequireResult> TargetAllocateCopyPageAsResult(vm_page_t* source,
                                                           DirtyState dirty_state,
                                                           VmCowPages::DeferredOps& deferred,
                                                           AnonymousPageRequest* page_request)
      TA_REQ(lock());

  // Attempts to turn the current cursor, which must be a reference, into a page.
  zx_status_t CursorReferenceToPage(AnonymousPageRequest* page_request) TA_REQ(lock());

  // Helpers for generating read or dirty requests for the given maximal range.
  zx_status_t ReadRequest(uint64_t max_request_pages, PageRequest* page_request) TA_REQ(lock());
  zx_status_t DirtyRequest(uint64_t max_request_pages, LazyPageRequest* page_request)
      TA_REQ(lock());

  // Target always exists. This is provided in the constructor and will always be non-null.
  VmCowPages* const target_;

  // The current offset_ in target_. This will always be <= end_offset_ and is only allowed to
  // increase. The validity of this range is checked prior to construction by GetLookupCursor
  uint64_t offset_ = 0;

  // The offset_ in target_ at which the cursor ceases being valid. The end_offset_ itself will
  // never be used as a valid offset_. VMOs are designed such that the end of a VMO+1 will not
  // overflow.
  const uint64_t end_offset_;

  // Captures information about the cursor owner. The different fields can be interpreted as
  // follows.
  //
  // owner_info_.cursor:
  // Cursor in the page list of the current owner_info_.owner or target_, depending on who owns the
  // page. Is only valid if is_valid_ is true. This is used to efficiently pull contiguous pages in
  // the owner and the current() value of it is cached in owner_cursor_.
  //
  // owner_info_.owner:
  // Represents the current owner of owner_cursor_/owner_info_.cursor. Can be non-null while
  // owner_info_.cursor is null to indicate a lack of content, although in this case the
  // owner can also be assumed to be the root. If owner_info_.owner is null while is_valid_ is true,
  // target_ is the owner of the cursor.
  //
  // owner_info_.owner_offset:
  // The offset_ normalized to the current owner. This is equal to offset_ when TargetIsOwner().
  //
  // owner_info_.visible_end:
  // Tracks the offset in target_ at which the current owner_info_.cursor becomes invalid. This
  // range essentially means that no VMO between target_ and owner_info_.owner had any content, and
  // so the cursor in owner is free to walk contiguous pages up to this point. This does not mean
  // that there is no content in the parent_ of the owner, and so even if owner_info_.visible_end is
  // not reached, if an empty slot is found the parent_ must then be checked. See IncrementCursor
  // for more details.
  PageLookup owner_info_;

  // This is a cache of owner_info_.cursor.current()
  VmPageOrMarkerRef owner_cursor_;

  // Value of target_->page_source_type() == PageSourceType::UserPager cached on
  // creation as there is spare padding space to store it here, and needed to retrieve this value to
  // initialize zero_fork_ anyway.
  const bool target_directly_backed_by_user_pager_;

  // Tracks whether zero forks should be tracked and placed in the corresponding page queue. This is
  // initialized to true if it's legal to place pages in the zero fork queue, which requires that
  // target_ not be pager backed.
  bool zero_fork_ = false;

  // Whether existing pages should be have their access time updated when they are returned.
  bool mark_accessed_ = true;

  // Whether the cursor is valid. The owner_info_ can only be used if is_valid_ is true, otherwise
  // it needs to be computed with EstablishCursor().
  bool is_valid_ = false;

  // Optional allocation list that will be used for any page allocations.
  VmPageDoublyLinkedList* alloc_list_ = nullptr;

  friend VmCowPages;
};

class ScopedPageFreedList {
 public:
  explicit ScopedPageFreedList() = default;

  ~ScopedPageFreedList() { ASSERT(list_.is_empty()); }

  void FreePages(VmCowPages* cow_pages) {
    if (!list_.is_empty()) {
      cow_pages->FreePages(&list_);
    }
    if (flph_.has_value()) {
      Pmm::Node().FinishFreeLoanedPages(*flph_);
    }
  }
  VmPageDoublyLinkedList* List() { return &list_; }
  FreeLoanedPagesHolder& Flph() {
    if (!flph_.has_value()) {
      flph_.emplace();
    }
    return *flph_;
  }

 private:
  VmPageDoublyLinkedList list_;
  // The FLPH is a moderately large object and is wrapped in an optional to defer its construction
  // unless it is actually needed.
  ktl::optional<FreeLoanedPagesHolder> flph_;
};

// Helper object for finishing VmCowPages operations that must occur after the lock is dropped. This
// is necessary due to some operations being externally locked. It is expected that this object is
// stack allocated using the __UNINITIALIZED tag in a sequence like this:
//
//     __UNINITIALIZED VmCowPages::DeferredOps deferred(cow_object_);
//     Guard<CriticalMutex> guard{cow_object_->lock()};
//     cow_object_->DoOperationLocked(&deferred);
//
// The destruction order will then allow |deferred| to perform its actions after |guard| is
// destructed and the lock is dropped.
// This class it not thread safe.
class VmCowPages::DeferredOps {
 public:
  // Construct a DeferredOps for the given VmCowPages. Must be constructed, and deconstructed,
  // without the lock held. It is the callers responsibility to ensure the pointer remains valid
  // over the lifetime of the object.
  explicit DeferredOps(VmCowPages* self) TA_EXCL(self->lock());
  ~DeferredOps();

  DeferredOps(const DeferredOps&) = delete;
  DeferredOps(DeferredOps&&) = delete;
  DeferredOps& operator=(const DeferredOps&) = delete;
  DeferredOps& operator=(DeferredOps&&) = delete;

 private:
  // Methods are private as they are only intended for use by the VmCowPages and not the external
  // caller holding this object on their stack.
  friend VmCowPages;
  friend PriorityChanger;

  // Indicate that the given range change operation should be performed later. Multiple ranges can
  // be specified, although only a single range that covers all of them will actually be invalidated
  // later, and the requested ops must all be the same (a mix of Unmap and UnmapZeroPage can be
  // given, with the entire operation upgraded to Unmap).
  void AddRange(VmCowPages* self, VmCowRange range, RangeChangeOp op);

  // Retrieves the underlying resource containers. Any pages (loaned or otherwise) that are added
  // will be freed *after* any range change operations are first performed.
  ScopedPageFreedList& FreedList(VmCowPages* self) {
    DEBUG_ASSERT(self == self_);
    return freed_list_;
  }

  // A reference to the VmCowPages for any deferred operations to be run against.
  VmCowPages* const self_;

  // Track any potential range change update that should be run over the cow children.
  struct DeferredRangeOp {
    RangeChangeOp op;
    VmCowRange range;
  };
  ktl::optional<DeferredRangeOp> range_op_;

  // Track any resources that need to be freed after the range change update.
  ScopedPageFreedList freed_list_;

  // When operating on a VMO from a hierarchy that has a page source the page source lock is held
  // over both the operation and our deferred operations. This serves to serialize operations
  // against all VMOs in the hierarchy. This serialization is necessary since a hierarchy with a
  // page source has parent VMOs whose contents is able to change, and if we had parallelism
  // between multiple mutating operations with range change updates user space would be able to see
  // inconsistent views of memory.
  // In addition to the lock itself, held via its Guard, we also hold a RefPtr to the PageSource
  // itself. During the lifetime of the DeferredOps it is possible for the self vmo to become
  // detached from the rest of the vmo tree, and for the remainder of the tree, including the root
  // node with the page source to be destroyed. Holding a RefPtr to the page source of the mutex we
  // are holding therefore prevents a use-after-free of the guard.
  ktl::optional<ktl::pair<Guard<Mutex>, fbl::RefPtr<PageSource>>> page_source_lock_;
};

// PriorityChanger is a transaction object for changing the high priority count of VmCowPages
// objects. The high priority count is used to manage page reclamation. This object ensures that a
// DeferredOps object is created against the correct VmCowPages, which might be an ancestor of
// |target_|. PriorityChanger does the following:
//
// * Manages the lifetime of VmCowPages to ensure they last until the
// priority change operation, by holding a RefPtr.
// * Handles the construction of the DeferredOps object.
// * Clarifies which operations need to be done within the VmCowPages lock and which need to happen
// without the lock being held.
// * Presents a high-level API to VmMapping and VmObjectPaged.
// * Handles traversing up the VmCowPages hierarchy to ensure that high priority status is
// propagated.
//
// Callers should prefer |VmMapping::SetMemoryPriorityLocked|,
// |VmMapping::SetMemoryPriorityDefaultLockedObject|, or
// |VmMapping::SetMemoryPriorityHighAlreadyPositiveLockedObject|, if applicable.
//
// To change the high priority count of a VmCowPages, you should first construct the PriorityChanger
// object. Then,
// * If |delta| < 0, you must immediately call one of the Change* methods, which change the high
// priority count of |target|.
// * If |delta| > 0, you must call an applicable Prepare* method to set up the prerequisites for the
// priority change operation, then call one of the Change* methods.
//
// This class is not thread-safe.
class PriorityChanger {
 public:
  // Construct a priority changer to change the high priority count of |target|. When |delta| is
  // positive, we go towards high priority. When it's negative, we move towards default
  // priority.
  //
  // The constructor may be called with or without |target|'s lock being held.
  //
  // The caller must ensure that |target| remains valid during the lifetime of PriorityChanger.
  PriorityChanger(int64_t delta, VmCowPages* target);

  ~PriorityChanger() {
    // Setting up a PriorityChanger without using it is wasteful.
    DEBUG_ASSERT(state_ == PriorityChangerState::DONE);
  }

  DISALLOW_COPY_ASSIGN_AND_MOVE(PriorityChanger);

  Lock<CriticalMutex>* lock() const TA_RET_CAP(target_->lock()) { return target_->lock(); }
  Lock<CriticalMutex>& lock_ref() const TA_RET_CAP(target_->lock()) { return target_->lock_ref(); }

  // This method may be called if |target|'s root page source has no loaned pages. Prefer other
  // Prepare* methods as this is typically hard to reason about.
  //
  // If you call this method, please leave a comment explaining how you know there are no loaned
  // pages in the current VmObject hierarchy.
  //
  // This method must be called with |target|'s lock held.
  void PrepareHasNoLoanedPagesLocked() TA_REQ(lock());

  // This method may be called if |target| already has a |high_priority_count_| > 0.
  //
  // This method must be called with |target|'s lock held.
  void PrepareIsAlreadyHighPriorityLocked() TA_REQ(lock());

  // This method must be called *without* holding |target|'s VmCowPages lock. In addition, the
  // caller must not hold a PriorityChanger or DeferredOps on any other VmCowPages in |target|'s
  // hierarchy.
  void PrepareMayNotAlreadyBeHighPriority() TA_EXCL(lock());

  // Increments or decrements the priority count of this VMO. The high priority count is used to
  // control any page reclamation, and applies to the whole VMO, including its parents. The count is
  // never allowed to go negative and so callers must only subtract what they have already added.
  // Further, callers are required to remove any additions before the VMO is destroyed.
  void ChangeHighPriorityCountLocked() TA_REQ(lock());

  // Internal (to VmCowPages) helper for modifying just this value of |high_priority_count_| without
  // performing any propagating.
  void ChangeSingleHighPriorityCountLocked() TA_REQ(lock());

 private:
  enum class PriorityChangerState : uint8_t {
    // User must call one of the Prepare* methods next.
    CONSTRUCTED,
    // User can call one of the Change* methods next.
    READY_TO_CHANGE,
    // User already called Change* so the only thing we can do is call the destructor.
    DONE
  };

  // Helper for PriorityChanger::ChangeSingleHighPriorityCountLocked and
  // PriorityChanger::ChangeHighPriorityCountLocked. Returns a new delta for whatever priority
  // change needs to be applied to the parent. With a zero return value, propagation must be
  // halted.
  //
  // This method is called with |current| starting as |target_|, and may eventually be called with
  // |current| as |root_|.
  //
  // Provide root_deferred whenever there may be loaned pages in the root.
  //
  // This method is static so that we can keep delta_ const, and propagate the change by using the
  // return value.
  static int64_t ChangeSingleHighPriorityCountLockedHelper(
      VmCowPages& current, int64_t delta, ktl::optional<VmCowPages::DeferredOps>& root_deferred)
      TA_REQ(current.lock());

  PriorityChangerState state_ = PriorityChangerState::CONSTRUCTED;
  // When delta_ > 0, move towards high priority, when delta_ < 0, move towards default priority.
  const int64_t delta_;
  // This must be present whenever root_deferred_ is present, since we are holding the VmCowPages to
  // ensure it stays live while root_deferred_ is live.
  fbl::RefPtr<VmCowPages> root_;
  // Always non-null.
  VmCowPages* const target_;
  // Must be present when there are loaned pages.
  ktl::optional<VmCowPages::DeferredOps> root_deferred_;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_COW_PAGES_H_
