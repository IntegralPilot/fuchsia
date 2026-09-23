// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PAGED_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PAGED_H_

#include <assert.h>
#include <lib/page/size.h>
#include <lib/user_copy/user_ptr.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/array.h>
#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/macros.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/mutex.h>
#include <kernel/range_check.h>
#include <vm/page_source.h>
#include <vm/pmm.h>
#include <vm/stream_size_manager.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>
#include <vm/vm_cow_pages.h>
#include <vm/vm_object.h>

class VmObjectPaged;
extern "C" void cpp_vm_object_paged_free(VmObjectPaged* vmo);

// the main VM object type, based on a copy-on-write set of pages.
class VmObjectPaged final : public VmObject, public VmDeferredDeleter<VmObjectPaged> {
 public:
  // |VmObject::options_| bitmask is extended with:
  static constexpr uint32_t kResizable = (1u << 1);
  static constexpr uint32_t kContiguous = (1u << 2);
  static constexpr uint32_t kSlice = (1u << 3);
  static constexpr uint32_t kDiscardable = (1u << 4);
  static constexpr uint32_t kAlwaysPinned = (1u << 5);
  static constexpr uint32_t kReference = (1u << 6);
  static constexpr uint32_t kCanBlockOnPageRequests = (1u << 31);

  Lock<CriticalMutex>* lock() const override TA_RET_CAP(cow_pages_->lock_ref()) {
    return cow_pages_->lock();
  }
  Lock<CriticalMutex>& lock_ref() const override TA_RET_CAP(cow_pages_->lock_ref()) {
    return cow_pages_->lock_ref();
  }
  uint64_t lock_order() const { return cow_pages_->lock_order(); }

  VmObject* self_locked() TA_REQ(lock()) TA_ASSERT(self_locked()->lock()) { return this; }
  const VmObject* self_locked() const TA_REQ(lock()) TA_ASSERT(self_locked()->lock()) {
    return this;
  }

  static zx_status_t Create(uint32_t pmm_alloc_flags, uint32_t options, uint64_t size,
                            fbl::RefPtr<VmObjectPaged>* vmo);

  // Create a VMO backed by a contiguous range of physical memory.  The
  // returned vmo has all of its pages committed, and does not allow
  // decommitting them.
  static zx_status_t CreateContiguous(uint32_t pmm_alloc_flags, uint64_t size,
                                      uint8_t alignment_log2, fbl::RefPtr<VmObjectPaged>* vmo);

  // Creates a VMO from wired pages.
  //
  // Creating a VMO using this method is destructive. Once the VMO is released, its
  // pages will be released into the general purpose page pool, so it is not possible
  // to create multiple VMOs for the same region using this method.
  //
  // |exclusive| indicates whether or not the created vmo should have exclusive access to
  // the pages. If exclusive is true, then [data, data + size) will be unmapped from the
  // kernel address space (unless they lie in the physmap).
  static zx_status_t CreateFromWiredPages(const void* data, size_t size, bool exclusive,
                                          fbl::RefPtr<VmObjectPaged>* vmo);

  static zx_status_t CreateExternal(fbl::RefPtr<PageSource> src, uint32_t options, uint64_t size,
                                    fbl::RefPtr<VmObjectPaged>* vmo);

  zx_status_t Resize(uint64_t size) override;

  uint64_t size_locked() const override TA_REQ(lock()) {
    // If this VmObject has a limit to the pages it references from |cow_pages_|, then that limit
    // determines the size of this object rather than the size of the whole |cow_pages_| object.
    return ktl::min(cow_pages_locked()->size_locked(), cow_range_.len);
  }

  // Queries the user defined stream size, which is distinct from the VMO size. Stream size is
  // byte-aligned and is not guaranteed to be in the range of the VMO. The lock does not not guard
  // the user changing the value via a syscall, so multiple calls under the same lock acquisition
  // can have different results.
  ktl::optional<uint64_t> user_stream_size_locked() TA_REQ(lock()) {
    if (!user_stream_size_) {
      return ktl::nullopt;
    }

    return user_stream_size_->GetStreamSize();
  }

  // Calculates the minimum of the VMO size and the page-aligned user stream size.
  ktl::optional<uint64_t> saturating_stream_size_locked() TA_REQ(lock()) {
    if (!user_stream_size_) {
      return ktl::nullopt;
    }

    uint64_t user_stream_size = user_stream_size_->GetStreamSize();
    uint64_t vmo_size = size_locked();

    // If user stream size is larger, trim to the VMO.
    // TODO(https://fxbug.dev/380960681): remove check when stream size <= VMO size invariant is
    // enforced.
    if (user_stream_size > vmo_size) {
      return vmo_size;
    }

    return RoundUpPageSize(user_stream_size);
  }

  bool is_contiguous() const override { return (options_ & kContiguous); }
  bool is_resizable() const override { return (options_ & kResizable); }
  bool is_discardable() const override { return (options_ & kDiscardable); }
  bool is_user_pager_backed() const override {
    return cow_pages_->is_root_source_user_pager_backed();
  }
  bool is_dirty_tracked() const override { return cow_pages_->is_dirty_tracked(); }
  // Streams are not supported on physical or contiguous VMOs.
  bool is_stream_compatible() const override { return is_paged() && !is_contiguous(); }

  void mark_modified_locked() override TA_REQ(lock()) {
    return cow_pages_locked()->mark_modified_locked();
  }
  ChildType child_type() const override {
    // Slices are implemented as references internally so for the purposes of reporting the
    // expected type back to the user the slice check must be done before the plain reference check.
    if (is_slice()) {
      return ChildType::kSlice;
    }
    if (is_reference()) {
      return ChildType::kReference;
    }
    Guard<CriticalMutex> guard{ChildListLock::Get()};
    return parent_ ? ChildType::kCowClone : ChildType::kNotChild;
  }
  bool is_slice() const { return options_ & kSlice; }
  bool is_reference() const { return (options_ & kReference); }
  uint64_t parent_user_id() const override TA_NO_THREAD_SAFETY_ANALYSIS {
    Guard<CriticalMutex> guard{ChildListLock::Get()};
    if (parent_) {
      return parent_->user_id();
    }
    return 0;
  }

  uint64_t HeapAllocationBytes() const override {
    Guard<CriticalMutex> guard{lock()};
    return cow_pages_locked()->HeapAllocationBytesLocked();
  }

  uint64_t ReclamationEventCount() const override {
    Guard<CriticalMutex> guard{lock()};

    return cow_pages_locked()->ReclamationEventCountLocked();
  }

  AttributionCounts GetAttributedMemoryInRange(uint64_t offset_bytes,
                                               uint64_t len_bytes) const override {
    Guard<CriticalMutex> guard{lock()};
    return GetAttributedMemoryInRangeLocked(offset_bytes, len_bytes);
  }

  AttributionCounts GetAttributedMemoryInReferenceOwner() const override {
    DEBUG_ASSERT(is_reference());
    Guard<CriticalMutex> guard{lock()};
    auto cow_range = GetCowRange(0, size_locked());
    DEBUG_ASSERT(cow_range.has_value());
    return cow_pages_locked()->GetAttributedMemoryInRangeLocked(cow_range.value());
  }

  AttributionCounts GetAttributedMemoryRangeInReferenceOwner(uint64_t offset, uint64_t len) const {
    DEBUG_ASSERT(is_reference());
    Guard<CriticalMutex> guard{lock()};
    return cow_pages_locked()->GetAttributedMemoryInRangeLocked(VmCowRange(offset, len));
  }

  // Does this VMO have any children that can see pages owned by this VMO (i.e. pages held by its
  // VmCowPages page list).
  bool children_cannot_access_pages() const {
    // If this is a reference whose parent has gone away, then the reference_list_ can contain
    // sibling references. Only consider actual children for this check.
    DEBUG_ASSERT(!is_reference());
    Guard<CriticalMutex> guard{lock()};
    // There are two cases to consider here:
    // - If there is a hidden node above this node in the parent chain, this node can only be a leaf
    // in the VmCowPages hierarchy tree. So its page list can only be seen by any references/slices,
    // hence the check for reference_list_.is_empty(). cow_pages_locked()->has_no_children_locked()
    // will always be true for this case.
    // - If there is no hidden node, then a CoW clone can exist below this node and can see its
    // pages, so we need to check for all types of children, both children represented in the CoW
    // tree, and also the reference list.
    return reference_list_.is_empty() && cow_pages_locked()->has_no_children_locked();
  }

  zx_status_t CommitRange(uint64_t offset, uint64_t len) override {
    return CommitRangeInternal(offset, len, /*pin=*/false, /*write=*/false);
  }
  zx_status_t CommitRangePinned(uint64_t offset, uint64_t len, bool write) override {
    return CommitRangeInternal(offset, len, /*pin=*/true, write);
  }
  zx_status_t PrefetchRange(uint64_t offset, uint64_t len) override;
  zx_status_t DecommitRange(uint64_t offset, uint64_t len) override;
  zx_status_t ZeroRange(uint64_t offset, uint64_t len) override {
    return ZeroRangeInternal(offset, len, /*dirty_track=*/true);
  }
  zx_status_t ZeroRangeUntracked(uint64_t offset, uint64_t len) override {
    // We don't expect any committed pages to remain at the end of this call, so we should be
    // operating on whole pages.
    if (!IsPageRounded(offset) || !IsPageRounded(len)) {
      return ZX_ERR_INVALID_ARGS;
    }
    return ZeroRangeInternal(offset, len, /*dirty_track=*/false);
  }

  void Unpin(uint64_t offset, uint64_t len) override {
    __UNINITIALIZED VmCowPages::DeferredOps deferred(cow_pages_.get());
    Guard<CriticalMutex> guard{lock()};
    auto cow_range = GetCowRange(offset, len);
    ASSERT(cow_range);
    cow_pages_locked()->UnpinLocked(*cow_range, &deferred);
  }

  // See VmObject::DebugIsRangePinned
  bool DebugIsRangePinned(uint64_t offset, uint64_t len) override {
    Guard<CriticalMutex> guard{lock()};
    if (auto cow_range = GetCowRange(offset, len)) {
      return cow_pages_locked()->DebugIsRangePinnedLocked(*cow_range);
    }
    return false;
  }

  zx_status_t LockRange(uint64_t offset, uint64_t len,
                        zx_vmo_lock_state_t* lock_state_out) override;
  zx_status_t TryLockRange(uint64_t offset, uint64_t len) override;
  zx_status_t UnlockRange(uint64_t offset, uint64_t len) override;
  zx_status_t Read(void* ptr, uint64_t offset, size_t len) override;
  zx_status_t Write(const void* ptr, uint64_t offset, size_t len) override;
  zx_status_t Lookup(uint64_t offset, uint64_t len, VmObject::LookupFunction lookup_fn) override;
  zx_status_t LookupContiguous(uint64_t offset, uint64_t len, paddr_t* out_paddr) override;

  ktl::pair<zx_status_t, size_t> ReadUser(user_out_ptr<char> ptr, uint64_t offset, size_t len,
                                          VmObjectReadWriteOptions options) override;
  ktl::pair<zx_status_t, size_t> WriteUser(
      user_in_ptr<const char> ptr, uint64_t offset, size_t len, VmObjectReadWriteOptions options,
      const OnWriteBytesTransferredCallback& on_bytes_transferred) override;
  ktl::pair<zx_status_t, size_t> ReadUserVector(user_out_iovec_t vec, uint64_t offset, size_t len);
  ktl::pair<zx_status_t, size_t> WriteUserVector(
      user_in_iovec_t vec, uint64_t offset, size_t len,
      const OnWriteBytesTransferredCallback& on_bytes_transferred);

  zx_status_t TakePages(uint64_t offset, uint64_t len, VmPageSpliceList* pages) override;
  zx_status_t SupplyPages(uint64_t offset, uint64_t len, VmPageSpliceList* pages,
                          SupplyOptions options) override;
  zx_status_t FailPageRequests(uint64_t offset, uint64_t len, zx_status_t error_status) override {
    Guard<CriticalMutex> guard{lock()};
    if (auto cow_range = GetCowRange(offset, len)) {
      return cow_pages_locked()->FailPageRequestsLocked(*cow_range, error_status);
    }
    return ZX_ERR_OUT_OF_RANGE;
  }

  zx_status_t DirtyPages(uint64_t offset, uint64_t len) override;
  zx_status_t EnumerateDirtyRanges(uint64_t offset, uint64_t len,
                                   DirtyRangeEnumerateFunction&& dirty_range_fn) override;

  zx_status_t QueryPagerVmoStats(bool reset, zx_pager_vmo_stats_t* stats) override {
    Guard<CriticalMutex> guard{lock()};
    return cow_pages_locked()->QueryPagerVmoStatsLocked(reset, stats);
  }

  void ResetPagerVmoStats() {
    Guard<CriticalMutex> guard{lock()};
    cow_pages_locked()->ResetPagerVmoStatsLocked();
  }

  zx_status_t WritebackBegin(uint64_t offset, uint64_t len, bool is_zero_range) override {
    Guard<CriticalMutex> guard{lock()};
    if (auto cow_range = GetCowRange(offset, len)) {
      return cow_pages_locked()->WritebackBeginLocked(*cow_range, is_zero_range);
    }
    return ZX_ERR_OUT_OF_RANGE;
  }
  zx_status_t WritebackEnd(uint64_t offset, uint64_t len) override {
    Guard<CriticalMutex> guard{lock()};
    if (auto cow_range = GetCowRange(offset, len)) {
      return cow_pages_locked()->WritebackEndLocked(*cow_range);
    }
    return ZX_ERR_OUT_OF_RANGE;
  }

  // See VmObject::SetUserStreamSize
  void SetUserStreamSize(fbl::RefPtr<StreamSizeManager> ssm) override {
    Guard<CriticalMutex> guard{lock()};
    user_stream_size_ = ktl::move(ssm);
  }

  void Dump(uint depth, bool verbose) override {
    Guard<CriticalMutex> guard{lock()};
    DumpLocked(depth, verbose);
  }

  uint32_t DebugLookupDepth() const override {
    Guard<CriticalMutex> guard{lock()};
    return cow_pages_locked()->DebugLookupDepthLocked();
  }

  zx_status_t GetPage(uint64_t offset, uint pf_flags, MultiPageRequest* page_request,
                      vm_page_t** page, paddr_t* pa) override;

  // Gets a reference to a LookupCursor for the specified range in the VMO. The returned cursor and
  // all the items returned by the cursor are only valid as long as the lock is contiguous held.
  //
  // The requested range must be fully inside the VMO, and if a cursor is returned the caller can
  // assume that all offsets up to |max_len| can be queried. Pages returned by the cursor itself
  // might be from this VMO or a parent, depending on actual cursor methods used.
  //
  // See |VmCowPages::LookupCursor|.
  zx::result<VmCowPages::LookupCursor> GetLookupCursorLocked(uint64_t offset, uint64_t max_len)
      TA_REQ(lock()) {
    auto cow_range = GetCowRange(offset, max_len);
    if (likely(cow_range)) {
      return cow_pages_locked()->GetLookupCursorLocked(*cow_range);
    }
    return zx::error{ZX_ERR_OUT_OF_RANGE};
  }

  zx_status_t CreateClone(Resizability resizable, SnapshotType type, uint64_t offset, uint64_t size,
                          bool copy_name, fbl::RefPtr<VmObject>* child_vmo) override;

  zx_status_t CacheOp(uint64_t offset, uint64_t len, CacheOpType type) override;

  arch_mmu_flags_t GetMappingCachePolicyLocked() const TA_REQ(lock()) {
    return self_locked()->GetMappingCachePolicyLocked();
  }
  zx_status_t SetMappingCachePolicy(arch_mmu_flags_t cache_policy) override;

  void DetachSource() override { cow_pages_->DetachSource(); }

  ktl::optional<zx_koid_t> GetPageSourceKoid() const override {
    if (is_reference()) {
      return ktl::nullopt;
    }
    return cow_pages_->GetPageSourceKoid();
  }

  zx_status_t CreateChildSlice(uint64_t offset, uint64_t size, bool copy_name,
                               fbl::RefPtr<VmObject>* child_vmo) override;

  zx_status_t CreateChildReference(Resizability resizable, uint64_t offset, uint64_t size,
                                   bool copy_name, bool* first_child,
                                   fbl::RefPtr<VmObject>* child_vmo) override;

  // Returns whether or not zero pages can be safely deduped from this VMO. Zero pages cannot be
  // deduped if the VMO is in use for kernel mappings, or if the pages cannot be accessed from the
  // physmap due to not being cached.
  bool CanDedupZeroPagesLocked() TA_REQ(lock());

  // Exposed for testing.
  fbl::RefPtr<VmCowPages> DebugGetCowPages() const {
    Guard<CriticalMutex> guard{lock()};
    return cow_pages_;
  }

  vm_page_t* DebugGetPage(uint64_t offset) const {
    Guard<CriticalMutex> guard{lock()};
    if (auto cow_range = GetCowRange(offset, kPageSize)) {
      return cow_pages_locked()->DebugGetPageLocked(cow_range->offset);
    }
    return nullptr;
  }

  // Apply the specified operation to all mappings in the given range.
  //
  // Preconditions
  // * This range may not include pages mapped in the kernel address space without the
  //   VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING flag.
  // * |range| must be page-aligned.
  void RangeChangeUpdateLocked(VmCowRange range, RangeChangeOp op) TA_REQ(lock());

  // Apply the specified operation to all mappings in the given range, forwarded to the original
  // owner of the VmCowPages. In the case of references and slices, this ensures that all VMOs in
  // the reference list of the original, cloned VMO are included.
  //
  // |offset| and |len| must be page-rounded.
  void ForwardRangeChangeUpdateLocked(uint64_t offset, uint64_t len, RangeChangeOp op)
      TA_REQ(lock());

  // Hint how the specified range is intended to be used, so that the hint can be taken into
  // consideration when reclaiming pages under memory pressure (if applicable).
  zx_status_t HintRange(uint64_t offset, uint64_t len, EvictionHint hint) override;

  void CommitHighPriorityPages(uint64_t offset, uint64_t len) override;

  // See the comments on |PriorityChanger| for how to use this object.
  PriorityChanger MakePriorityChanger(int64_t delta) {
    return PriorityChanger(delta, cow_pages_.get());
  }

  void MaybeDeadTransition() {}

  // Constructs and returns a |DeferredOps| that can be passed into other methods on this VMO that
  // require one.
  VmCowPages::DeferredOps MakeDeferredOps() { return VmCowPages::DeferredOps(cow_pages_.get()); }

 private:
  // private constructor (use Create())
  VmObjectPaged(uint32_t options, fbl::RefPtr<VmCowPages> cow_pages);
  VmObjectPaged(uint32_t options, fbl::RefPtr<VmCowPages> cow_pages, VmCowRange range);

  static zx_status_t CreateCommon(uint32_t pmm_alloc_flags, uint32_t options, uint64_t size,
                                  fbl::RefPtr<VmObjectPaged>* vmo);
  static zx_status_t CreateWithSourceCommon(fbl::RefPtr<PageSource> src, uint32_t pmm_alloc_flags,
                                            uint32_t options, uint64_t size,
                                            fbl::RefPtr<VmObjectPaged>* obj);

  // private destructor, only called from refptr
  ~VmObjectPaged() override;
  friend fbl::RefPtr<VmObjectPaged>;
  friend void ::cpp_vm_object_paged_free(VmObjectPaged* vmo);

  DISALLOW_COPY_ASSIGN_AND_MOVE(VmObjectPaged);

  // Helper for the destructor as by default destructors do not require locks to be held under the
  // assumption the destructor runs when no other entities can see the object. This is not true here
  // and locking is still relevant, so having the cleanup logic in a separate method allows for full
  // static analysis to happen.
  void DestructorHelper();

  zx_status_t CreateChildReferenceCommon(uint32_t options, VmCowRange range, bool allow_uncached,
                                         bool copy_name, bool* first_child,
                                         fbl::RefPtr<VmObject>* child_vmo);

  // Unified function that implements both CommitRange and CommitRangePinned
  zx_status_t CommitRangeInternal(uint64_t offset, uint64_t len, bool pin, bool write);

  // see GetAttributedMemoryInRange
  AttributionCounts GetAttributedMemoryInRangeLocked(uint64_t offset_bytes,
                                                     uint64_t len_bytes) const TA_REQ(lock());

  // internal read/write routine that takes a templated copy function to help share some code
  template <typename T>
  ktl::pair<zx_status_t, size_t> ReadWriteInternal(uint64_t offset, size_t len, bool write,
                                                   VmObjectReadWriteOptions options, T copyfunc);

  // Zeroes a partial range in a page. The page to zero is looked up using page_base_offset, and
  // will be committed if needed. The range of [zero_start_offset, zero_end_offset) is relative to
  // the page and so [0, kPageSize) would zero the entire page.
  zx_status_t ZeroPartialPage(uint64_t page_base_offset, uint64_t zero_start_offset,
                              uint64_t zero_end_offset);

  // Internal helper for ZeroRange*.
  zx_status_t ZeroRangeInternal(uint64_t offset, uint64_t len, bool dirty_track);

  // Internal implementations that assume lock is already held.
  void DumpLocked(uint depth, bool verbose) const TA_REQ(lock());

  // Convenience wrapper that returns cow_pages_ whilst asserting that the lock is held.
  VmCowPages* cow_pages_locked() const TA_REQ(lock()) TA_ASSERT(cow_pages_locked()->lock()) {
    AssertHeld(cow_pages_->lock_ref());
    return cow_pages_.get();
  }

  // Translate a range in this VmObject to a VmCowRange in |cow_pages_|.
  //
  // The translated range might extend beyond the end of the cow_pages_ object. This function will
  // return |ktl::nullopt| if the translated range might have included pages in cow_pages_ that
  // should not be referenced by this VmObject (e.g., if this VmObject is a slice reference).
  ktl::optional<VmCowRange> GetCowRange(uint64_t offset, uint64_t len) const {
    if (likely(InRange(offset, len, cow_range_.len))) {
      return VmCowRange(offset + cow_range_.offset, len);
    }
    return ktl::nullopt;
  }

  // Similar to GetCowRange, but also checks for being within the range of the cow pages size.
  ktl::optional<VmCowRange> GetCowRangeSizeCheckLocked(uint64_t offset, uint64_t len) const
      TA_REQ(lock()) {
    if (likely(InRange(offset, len, size_locked()))) {
      return VmCowRange(offset + cow_range_.offset, len);
    }
    return ktl::nullopt;
  }

  // This is a debug only state that is used to simplify assertions and validations around blocking
  // on page requests. If false no operations on this VMO will ever fill out the PageRequest
  // that is passed in, and will never block in ops like Commit that say they might block. This
  // creates a carve-out that is necessary as kernel internals need to call VMO operations that
  // might block on VMOs that they know won't block, and not have assertions spuriously trip. This
  // acts as the union of user pager backed VMOs, as well as VMOs that might wait on internal kernel
  // page sources.
  bool can_block_on_page_requests() const { return options_ & kCanBlockOnPageRequests; }

  using ReferenceListNodeState = fbl::DoublyLinkedListNodeState<VmObjectPaged*>;
  struct ReferenceListTraits {
    static ReferenceListNodeState& node_state(VmObjectPaged& vmo) {
      return vmo.reference_list_node_state_;
    }
  };
  friend struct ReferenceListTraits;
  ReferenceListNodeState reference_list_node_state_;
  using ReferenceList = fbl::DoublyLinkedListCustomTraits<VmObjectPaged*, ReferenceListTraits>;

  // list of every reference child
  ReferenceList reference_list_ TA_GUARDED(lock());

  // parent pointer (may be null). This is a raw pointer as we have no need to hold our parent alive
  // once they want to go away.
  VmObjectPaged* parent_ TA_GUARDED(ChildListLock::Get()) = nullptr;

  const fbl::RefPtr<VmCowPages> cow_pages_;

  // The range in |cow_pages_| that this VmObject references.
  //
  // This range can be less than the whole VmCowPage for a slice reference.
  const VmCowRange cow_range_;

  // A user supplied stream size that can be queried. By itself this has no semantic meaning and is
  // only read and used specifically when requested by the user. See VmObject::SetUserStreamSize.
  fbl::RefPtr<StreamSizeManager> user_stream_size_ TA_GUARDED(lock());
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PAGED_H_
