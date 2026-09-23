// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "vm/vm_object_paged.h"

#include <align.h>
#include <assert.h>
#include <inttypes.h>
#include <lib/console.h>
#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/page/size.h>
#include <stdlib.h>
#include <string.h>
#include <trace.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/ops.h>
#include <fbl/alloc_checker.h>
#include <ktl/algorithm.h>
#include <ktl/array.h>
#include <ktl/utility.h>
#include <vm/discardable_vmo_tracker.h>
#include <vm/fault.h>
#include <vm/page_source.h>
#include <vm/physical_page_provider.h>
#include <vm/physmap.h>
#include <vm/vm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_cow_pages.h>

#include "vm_priv.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE VM_GLOBAL_TRACE(0)

namespace {

KCOUNTER(vmo_attribution_queries, "vm.attributed_memory.object.queries")

}  // namespace

VmObjectPaged::VmObjectPaged(uint32_t options, fbl::RefPtr<VmCowPages> cow_pages, VmCowRange range)
    : VmObject(options | kPaged), cow_pages_(ktl::move(cow_pages)), cow_range_(range) {
  LTRACEF("%p\n", this);
}

VmObjectPaged::VmObjectPaged(uint32_t options, fbl::RefPtr<VmCowPages> cow_pages)
    : VmObjectPaged(options, ktl::move(cow_pages), VmCowRange(0, UINT64_MAX)) {}

VmObjectPaged::~VmObjectPaged() {
  canary_.Assert();

  LTRACEF("%p\n", this);

  // VmObjectPaged initialize must always complete and is not allowed to fail, as such it should
  // always end up in the global list.
  DEBUG_ASSERT(InGlobalList());

  DestructorHelper();
}

void VmObjectPaged::DestructorHelper() {
  RemoveFromGlobalList();

  if (unlikely(options_ & kAlwaysPinned)) {
    // It's possible that the final stage of construction of an always pinned vmo can fail, in which
    // case it might not have any pinned pages. To avoid unpinning a range that has no pinned pages
    // (which would panic), we first check if there are any pinned pages.
    bool unpin = true;
    {
      Guard<CriticalMutex> guard{lock()};
      if (unlikely(cow_pages_locked()->pinned_page_count_locked() == 0)) {
        // There's no pinned pages at all. This can happen if the VMO is empty or if the final stage
        // of construction failed, leaving committed-but-unpinned pages which will be cleaned up
        // naturally when cow_pages_ is destroyed. We don't need to unpin anything.
        unpin = false;
      }
    }
    if (likely(unpin)) {
      Unpin(0, size());
    }
  }

  fbl::RefPtr<VmCowPages> deferred;
  {
    Guard<CriticalMutex> guard{lock()};

    // Only clear the backlink if we are not a reference. A reference does not "own" the VmCowPages,
    // so in the typical case, the VmCowPages will not have its backlink set to a reference. There
    // does exist an edge case where the backlink can be a reference, which is handled by the else
    // block below.
    if (!is_reference()) {
      cow_pages_locked()->set_paged_backlink_locked(nullptr);
    } else {
      // If this is a reference, we need to remove it from the original (parent) VMO's reference
      // list.
      VmObjectPaged* root_ref = cow_pages_locked()->get_paged_backlink_locked();
      // The VmCowPages will have a valid backlink, either to the original VmObjectPaged or a
      // reference VmObjectPaged, as long as there is a reference that is alive. We know that this
      // is a reference.
      DEBUG_ASSERT(root_ref);
      if (likely(root_ref != this)) {
        AssertHeld(root_ref->lock_ref());
        VmObjectPaged* removed = root_ref->reference_list_.erase(*this);
        DEBUG_ASSERT(removed == this);
      } else {
        // It is possible for the backlink to point to |this| if the original parent went away at
        // some point and the rest of the reference list had to be re-homed to |this|, and the
        // backlink set to |this|. The VmCowPages was pointing to us, so clear the backlink. The
        // backlink will get reset below if other references remain.
        cow_pages_locked()->set_paged_backlink_locked(nullptr);
      }
    }

    // If this VMO had references, pick one of the references as the paged backlink from the shared
    // VmCowPages. Also, move the remainder of the reference list to the chosen reference. Note that
    // we're only moving the reference list over without adding the references to the children list;
    // we do not want these references to be counted as children of the chosen VMO. We simply want a
    // safe way to propagate mapping updates and VmCowPages changes on hidden node addition.
    if (!reference_list_.is_empty()) {
      // We should only be attempting to reset the backlink if the owner is going away and has reset
      // the backlink above.
      DEBUG_ASSERT(cow_pages_locked()->get_paged_backlink_locked() == nullptr);
      VmObjectPaged* paged_backlink = reference_list_.pop_front();
      cow_pages_locked()->set_paged_backlink_locked(paged_backlink);
      AssertHeld(paged_backlink->lock_ref());
      paged_backlink->reference_list_.splice(paged_backlink->reference_list_.end(),
                                             reference_list_);
    }
    DEBUG_ASSERT(reference_list_.is_empty());
    deferred = cow_pages_;
  }
  while (deferred) {
    deferred = deferred->MaybeDeadTransition();
  }

  fbl::RefPtr<VmObjectPaged> maybe_parent;

  // Re-home all our children with any parent that we have.
  {
    Guard<CriticalMutex> child_guard{ChildListLock::Get()};
    while (!children_list_.is_empty()) {
      VmObject* c = &children_list_.front();
      children_list_.pop_front();
      VmObjectPaged* child = reinterpret_cast<VmObjectPaged*>(c);
      child->parent_ = parent_;
      if (parent_) {
        // Ignore the return since 'this' is a child so we know we are not transitioning from 0->1
        // children.
        [[maybe_unused]] bool notify = parent_->AddChildLocked(child);
        DEBUG_ASSERT(!notify);
      }
    }

    if (parent_) {
      // As parent_ is a raw pointer we must ensure that if we call a method on it that it lives
      // long enough. To do so we attempt to upgrade it to a refptr, which could fail if it's
      // already slated for deletion.
      maybe_parent = fbl::MakeRefPtrUpgradeFromRaw(parent_, child_guard);
      if (maybe_parent) {
        // Holding refptr, can safely pass in the guard to RemoveChild.
        parent_->RemoveChild(this, child_guard.take());
      } else {
        // parent is up for deletion and so there's no need to use RemoveChild since there is no
        // user dispatcher to notify anyway and so just drop ourselves to keep the hierarchy
        // correct.
        parent_->DropChildLocked(this);
      }
    }
  }
  if (maybe_parent) {
    // As we constructed a RefPtr to our parent, and we are in our own destructor, there is now
    // the potential for recursive destruction if we need to delete the parent due to holding the
    // last ref, hit this same path, etc.
    VmDeferredDeleter<VmObjectPaged>::DoDeferredDelete(ktl::move(maybe_parent));
  }
}

zx_status_t VmObjectPaged::HintRange(uint64_t offset, uint64_t len, EvictionHint hint) {
  canary_.Assert();

  if (can_block_on_page_requests() && hint == EvictionHint::AlwaysNeed) {
    lockdep::AssertNoLocksHeld();
  }

  // Ignore hints for non user-pager-backed VMOs. We choose to silently ignore hints for
  // incompatible combinations instead of failing. This is because the kernel does not make any
  // explicit guarantees on hints; since they are just hints, the kernel is always free to ignore
  // them.
  if (!cow_pages_->can_root_source_evict()) {
    return ZX_OK;
  }

  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  switch (hint) {
    case EvictionHint::DontNeed: {
      return cow_pages_->PromoteRangeForReclamation(*cow_range);
    }
    case EvictionHint::AlwaysNeed: {
      // Hints are best effort, so ignore any errors in the paging in process.
      return cow_pages_->ProtectRangeFromReclamation(*cow_range, /*set_always_need=*/true,
                                                     /*ignore_errors=*/true);
    }
  }

  return ZX_OK;
}

zx_status_t VmObjectPaged::PrefetchRange(uint64_t offset, uint64_t len) {
  canary_.Assert();
  if (can_block_on_page_requests()) {
    lockdep::AssertNoLocksHeld();
  }

  // Round offset and len to be page aligned. Use a sub-scope to validate that temporary end
  // calculations cannot be accidentally used later on.
  {
    uint64_t end;
    if (add_overflow(offset, len, &end)) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    const uint64_t end_page = RoundUpPageSize(end);
    if (end_page < end) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    DEBUG_ASSERT(end_page >= offset);
    offset = RoundDownPageSize(offset);
    len = end_page - offset;
  }

  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  // Cannot overflow otherwise IsBoundedBy would have failed.
  DEBUG_ASSERT(cow_range->is_page_aligned());
  if (cow_pages_->is_root_source_user_pager_backed()) {
    return cow_pages_->ProtectRangeFromReclamation(*cow_range,
                                                   /*set_always_need=*/false,
                                                   /*ignore_errors=*/false);
  }
  // Committing high priority pages is best effort, so ignore any errors from decompressing.
  return cow_pages_->DecompressInRange(*cow_range);
}

void VmObjectPaged::CommitHighPriorityPages(uint64_t offset, uint64_t len) {
  {
    Guard<CriticalMutex> guard{lock()};
    if (!cow_pages_locked()->is_high_memory_priority_locked()) {
      return;
    }
  }
  // Ignore the result of the prefetch, high priority commit is best effort.
  PrefetchRange(offset, len);
}

bool VmObjectPaged::CanDedupZeroPagesLocked() {
  canary_.Assert();

  // Skip uncached VMOs as we cannot efficiently scan them.
  if ((self_locked()->GetMappingCachePolicyLocked() & ZX_CACHE_POLICY_MASK) !=
      ZX_CACHE_POLICY_CACHED) {
    return false;
  }

  // Okay to dedup from this VMO.
  return true;
}

zx_status_t VmObjectPaged::CreateCommon(uint32_t pmm_alloc_flags, uint32_t options, uint64_t size,
                                        fbl::RefPtr<VmObjectPaged>* obj) {
  DEBUG_ASSERT(!(options & (kContiguous | kCanBlockOnPageRequests)));

  // Cannot be resizable and pinned, otherwise we will lose track of the pinned range.
  if ((options & kResizable) && (options & kAlwaysPinned)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (pmm_alloc_flags & PMM_ALLOC_FLAG_CAN_WAIT) {
    options |= kCanBlockOnPageRequests;
  }

  // make sure size is page aligned
  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (size > MAX_SIZE) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  fbl::AllocChecker ac;

  ktl::unique_ptr<DiscardableVmoTracker> discardable = nullptr;
  if (options & kDiscardable) {
    discardable = ktl::make_unique<DiscardableVmoTracker>(&ac);
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
  }

  // This function isn't used to create slices or pager-backed VMOs, so VmCowPageOptions can be
  // kNone.
  fbl::RefPtr<VmCowPages> cow_pages;
  zx_status_t status = VmCowPages::Create(VmCowPagesOptions::kNone, pmm_alloc_flags, size,
                                          ktl::move(discardable), &cow_pages);
  if (status != ZX_OK) {
    return status;
  }

  auto vmo = fbl::AdoptRef<VmObjectPaged>(new (&ac) VmObjectPaged(options, ktl::move(cow_pages)));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  // This creation has succeeded. Must wire up the cow pages and *then* place in the globals list.
  {
    Guard<CriticalMutex> guard{vmo->lock()};
    vmo->cow_pages_locked()->set_paged_backlink_locked(vmo.get());
    vmo->cow_pages_locked()->TransitionToAliveLocked();
  }
  vmo->AddToGlobalList();

  // The object is now 'fully created' and can now fulfill any kAlwaysPinned request. Although this
  // allows anyone who is walking the AllVmosList to see this VMO prior to its pages being
  // committed, this is fine as they are not the user depending on the pinning. If this fails we can
  // still tear down the whole object and the VmObjectPaged destructor explicitly handles the case
  // of destruction without successfully committing and pinning our pages.
  if (options & kAlwaysPinned) {
    status = vmo->CommitRangePinned(0, size, /*write=*/true);
    if (status != ZX_OK) {
      return status;
    }
  }

  *obj = ktl::move(vmo);

  return ZX_OK;
}

zx_status_t VmObjectPaged::Create(uint32_t pmm_alloc_flags, uint32_t options, uint64_t size,
                                  fbl::RefPtr<VmObjectPaged>* obj) {
  if (options & (kContiguous | kCanBlockOnPageRequests)) {
    // Force callers to use CreateContiguous() instead.
    return ZX_ERR_INVALID_ARGS;
  }

  return CreateCommon(pmm_alloc_flags, options, size, obj);
}

zx_status_t VmObjectPaged::CreateContiguous(uint32_t pmm_alloc_flags, uint64_t size,
                                            uint8_t alignment_log2,
                                            fbl::RefPtr<VmObjectPaged>* obj) {
  DEBUG_ASSERT(alignment_log2 < sizeof(uint64_t) * 8);
  // make sure size is page aligned
  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (size > MAX_SIZE) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  fbl::AllocChecker ac;
  // For contiguous VMOs, we need a PhysicalPageProvider to reclaim specific loaned physical pages
  // on commit.
  auto page_provider = fbl::AdoptRef(new (&ac) PhysicalPageProvider(size));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  PhysicalPageProvider* physical_page_provider_ptr = page_provider.get();
  fbl::RefPtr<PageSource> page_source =
      fbl::AdoptRef(new (&ac) PageSource(ktl::move(page_provider)));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  auto* page_source_ptr = page_source.get();

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      CreateWithSourceCommon(page_source, pmm_alloc_flags, kContiguous, size, &vmo);
  if (status != ZX_OK) {
    // Ensure to close the page source we created, as it will not get closed by the VmCowPages since
    // that creation failed.
    page_source->Close();
    return status;
  }

  if (size == 0) {
    *obj = ktl::move(vmo);
    return ZX_OK;
  }

  VmPageDoublyLinkedList page_list;

  size_t num_pages = size / kPageSize;
  paddr_t pa;
  status = pmm_alloc_contiguous(num_pages, pmm_alloc_flags, alignment_log2, &pa, &page_list);
  if (status != ZX_OK) {
    LTRACEF("failed to allocate enough pages (asked for %zu)\n", num_pages);
    return ZX_ERR_NO_MEMORY;
  }
  Guard<CriticalMutex> guard{vmo->lock()};
  // Add them to the appropriate range of the object, this takes ownership of all the pages
  // regardless of outcome.
  // This is a newly created VMO with a page source, so we don't expect to be overwriting anything
  // in its page list.
  status = vmo->cow_pages_locked()->AddNewPagesLocked(
      0, &page_list, VmCowPages::CanOverwriteSlot::Empty, true, nullptr);
  if (status != ZX_OK) {
    return status;
  }

  physical_page_provider_ptr->Init(vmo->cow_pages_locked(), page_source_ptr, pa);

  *obj = ktl::move(vmo);
  return ZX_OK;
}

zx_status_t VmObjectPaged::CreateFromWiredPages(const void* data, size_t size, bool exclusive,
                                                fbl::RefPtr<VmObjectPaged>* obj) {
  LTRACEF("data %p, size %zu\n", data, size);

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = CreateCommon(PMM_ALLOC_FLAG_ANY, 0, size, &vmo);
  if (status != ZX_OK) {
    return status;
  }

  if (size > 0) {
    ASSERT(IsPageRounded(size));
    ASSERT(IsPageRounded(reinterpret_cast<uintptr_t>(data)));

    // Do a direct lookup of the physical pages backing the range of
    // the kernel that these addresses belong to and jam them directly
    // into the VMO.
    //
    // NOTE: This relies on the kernel not otherwise owning the pages.
    // If the setup of the kernel's address space changes so that the
    // pages are attached to a kernel VMO, this will need to change.

    paddr_t start_paddr = vaddr_to_paddr(data);
    ASSERT(start_paddr != 0);

    Guard<CriticalMutex> guard{vmo->lock()};

    for (size_t count = 0; count < size / kPageSize; count++) {
      paddr_t pa = start_paddr + count * kPageSize;
      vm_page_t* page = paddr_to_vm_page(pa);
      ASSERT(page);

      if (page->state() == vm_page_state::WIRED) {
        pmm_unwire_page(page);
      } else {
        // This function is only valid for memory in the boot image,
        // which should all be wired.
        panic("page used to back static vmo in unusable state: paddr %#" PRIxPTR " state %zu\n", pa,
              VmPageStateIndex(page->state()));
      }
      // This is a newly created anonymous VMO, so we expect to be overwriting zeros. A newly
      // created anonymous VMO with no committed pages has all its content implicitly zero.
      status = vmo->cow_pages_locked()->AddNewPageLocked(
          count * kPageSize, page, VmCowPages::CanOverwriteSlot::Empty, nullptr, false, nullptr);
      ASSERT_MSG(status == ZX_OK,
                 "AddNewPageLocked failed on page %zu of %zu at %#" PRIx64 " from [%#" PRIx64
                 ", %#" PRIx64 ")",
                 count, size / kPageSize, pa, start_paddr, start_paddr + size);
      DEBUG_ASSERT(!page->is_loaned());
    }

    if (exclusive && !is_physmap_addr(data)) {
      // unmap it from the kernel
      // NOTE: this means the image can no longer be referenced from original pointer
      status = VmAspace::kernel_aspace()->arch_aspace().Unmap(
          reinterpret_cast<vaddr_t>(data), size / kPageSize,
          ArchVmAspaceInterface::ArchUnmapOptions::None);
      ASSERT(status == ZX_OK);
    }
    if (!exclusive) {
      // Pin all the pages as we must never decommit any of them since they are shared elsewhere.
      ASSERT(vmo->cow_range_.offset == 0);
      status = vmo->cow_pages_locked()->PinRangeLocked(VmCowRange(0, size));
      ASSERT(status == ZX_OK);
    }
  }

  *obj = ktl::move(vmo);

  return ZX_OK;
}

zx_status_t VmObjectPaged::CreateExternal(fbl::RefPtr<PageSource> src, uint32_t options,
                                          uint64_t size, fbl::RefPtr<VmObjectPaged>* obj) {
  if (options & (kDiscardable | kCanBlockOnPageRequests | kAlwaysPinned)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // make sure size is page aligned
  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (size > MAX_SIZE) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // External VMOs always support delayed PMM allocations, since they already have to tolerate
  // arbitrary waits for pages due to the PageSource.
  return CreateWithSourceCommon(ktl::move(src), PMM_ALLOC_FLAG_ANY | PMM_ALLOC_FLAG_CAN_WAIT,
                                options | kCanBlockOnPageRequests, size, obj);
}

zx_status_t VmObjectPaged::CreateWithSourceCommon(fbl::RefPtr<PageSource> src,
                                                  uint32_t pmm_alloc_flags, uint32_t options,
                                                  uint64_t size, fbl::RefPtr<VmObjectPaged>* obj) {
  // Caller must check that size is page aligned.
  DEBUG_ASSERT(IsPageRounded(size));
  DEBUG_ASSERT(!(options & kAlwaysPinned));

  fbl::AllocChecker ac;

  // The cow pages will have a page source, so blocking is always possible.
  options |= kCanBlockOnPageRequests;

  VmCowPagesOptions cow_options = VmCowPagesOptions::kNone;
  cow_options |= VmCowPagesOptions::kPageSourceRoot;

  if (options & kContiguous) {
    cow_options |= VmCowPagesOptions::kCannotDecommitZeroPages;
  }

  if (src->properties().is_user_pager) {
    cow_options |= VmCowPagesOptions::kUserPagerBackedRoot;
  }

  fbl::RefPtr<VmCowPages> cow_pages;
  zx_status_t status = VmCowPages::CreateExternal(ktl::move(src), cow_options, size, &cow_pages);
  if (status != ZX_OK) {
    return status;
  }

  auto vmo = fbl::AdoptRef<VmObjectPaged>(new (&ac) VmObjectPaged(options, ktl::move(cow_pages)));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  // This creation has succeeded. Must wire up the cow pages and *then* place in the globals list.
  {
    Guard<CriticalMutex> guard{vmo->lock()};
    vmo->cow_pages_locked()->set_paged_backlink_locked(vmo.get());
    vmo->cow_pages_locked()->TransitionToAliveLocked();
  }
  vmo->AddToGlobalList();

  *obj = ktl::move(vmo);

  return ZX_OK;
}

zx_status_t VmObjectPaged::CreateChildSlice(uint64_t offset, uint64_t size, bool copy_name,
                                            fbl::RefPtr<VmObject>* child_vmo) {
  LTRACEF("vmo %p offset %#" PRIx64 " size %#" PRIx64 "\n", this, offset, size);

  canary_.Assert();

  // Offset must be page aligned.
  if (!IsPageRounded(offset)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Make sure size is page aligned.
  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (size > MAX_SIZE) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // Slice must be wholly contained. |size()| will read the size holding the lock. This extra
  // acquisition is correct as we must drop the lock in order to perform the allocations.
  VmCowRange range;
  {
    Guard<CriticalMutex> guard{lock()};
    auto cow_range = GetCowRangeSizeCheckLocked(offset, size);
    if (!cow_range) {
      return ZX_ERR_INVALID_ARGS;
    }
    range = *cow_range;
  }

  // Forbid creating children of resizable VMOs. This restriction may be lifted in the future.
  if (is_resizable()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  uint32_t options = kSlice;
  if (is_contiguous()) {
    options |= kContiguous;
  }

  // If this VMO is contiguous then we allow creating an uncached slice.  When zeroing pages that
  // are reclaimed from having been loaned from a contiguous VMO, we will zero the pages and flush
  // the zeroes to RAM.
  const bool allow_uncached = is_contiguous();
  return CreateChildReferenceCommon(options, range, allow_uncached, copy_name, nullptr, child_vmo);
}

zx_status_t VmObjectPaged::CreateChildReference(Resizability resizable, uint64_t offset,
                                                uint64_t size, bool copy_name, bool* first_child,
                                                fbl::RefPtr<VmObject>* child_vmo) {
  LTRACEF("vmo %p offset %#" PRIx64 " size %#" PRIx64 "\n", this, offset, size);

  canary_.Assert();

  // A reference spans the entirety of the parent. The specified range has no meaning, require it
  // to be zero.
  if (offset != 0 || size != 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (is_slice()) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  ASSERT(cow_range_.offset == 0);

  // Not supported for contiguous VMOs. Can use slices instead as contiguous VMOs are non-resizable
  // and support slices.
  if (is_contiguous()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (resizable == Resizability::Resizable) {
    // Cannot create a resizable reference from a non-resizable VMO.
    if (!is_resizable()) {
      return ZX_ERR_NOT_SUPPORTED;
    }
  }

  uint32_t options = 0;

  // Reference inherits resizability from parent.
  if (is_resizable()) {
    options |= kResizable;
  }

  return CreateChildReferenceCommon(options, VmCowRange(0, UINT64_MAX), false, copy_name,
                                    first_child, child_vmo);
}

zx_status_t VmObjectPaged::CreateChildReferenceCommon(uint32_t options, VmCowRange range,
                                                      bool allow_uncached, bool copy_name,
                                                      bool* first_child,
                                                      fbl::RefPtr<VmObject>* child_vmo) {
  canary_.Assert();

  options |= kReference;

  if (can_block_on_page_requests()) {
    options |= kCanBlockOnPageRequests;
  }

  // Reference shares the same VmCowPages as the parent.
  fbl::RefPtr<VmObjectPaged> vmo;
  {
    Guard<CriticalMutex> guard{lock()};

    // We know that we are not contiguous so we should not be uncached either.
    if (self_locked()->GetMappingCachePolicyLocked() != ARCH_MMU_FLAG_CACHED && !allow_uncached) {
      return ZX_ERR_BAD_STATE;
    }

    // Once all fallible checks are performed, construct the VmObjectPaged.
    fbl::AllocChecker ac;
    vmo = fbl::AdoptRef<VmObjectPaged>(new (&ac) VmObjectPaged(options, cow_pages_, range));
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
    AssertHeld(vmo->lock_ref());

    // There's no way good way to convince the static analysis that the vmo->lock() that we hold is
    // also the VmObject::lock() in vmo and so we disable analysis to set the cache_policy_;
    [&vmo, cache_policy = GetMappingCachePolicyLocked()]() TA_REQ(vmo->lock())
        TA_NO_THREAD_SAFETY_ANALYSIS { vmo->cache_policy_ = cache_policy; }();
    {
      Guard<CriticalMutex> child_guard{ChildListLock::Get()};
      vmo->parent_ = this;
      const bool first = AddChildLocked(vmo.get());
      if (first_child) {
        *first_child = first;
      }
    }

    // Also insert into the reference list. The reference should only be inserted in the list of the
    // object that the cow_pages_locked() has the backlink to, i.e. the notional "owner" of the
    // VmCowPages.
    // As a consequence of this, in the case of nested references, the reference relationship can
    // look different from the parent->child relationship, which instead mirrors the child creation
    // calls as specified by the user (this is true for all child types).
    VmObjectPaged* paged_owner = cow_pages_locked()->get_paged_backlink_locked();
    // The VmCowPages we point to should have a valid backlink, either to us or to our parent (if we
    // are a reference).
    DEBUG_ASSERT(paged_owner);
    // If this object is not a reference, the |paged_owner| we computed should be the same as
    // |this|.
    DEBUG_ASSERT(is_reference() || paged_owner == this);
    AssertHeld(paged_owner->lock_ref());
    paged_owner->reference_list_.push_back(vmo.get());

    if (copy_name) {
      // There's no way good way to convince the static analysis that the vmo->lock() that we hold
      // is also the VmObject::lock() in vmo and so we disable analysis to set the name;
      [&vmo, &name = name_]() TA_REQ(vmo->lock()) TA_REQ(lock())
          TA_NO_THREAD_SAFETY_ANALYSIS { vmo->name_ = name; }();
    }
  }

  // Add to the global list now that fully initialized.
  vmo->AddToGlobalList();

  *child_vmo = ktl::move(vmo);

  return ZX_OK;
}

zx_status_t VmObjectPaged::CreateClone(Resizability resizable, SnapshotType type, uint64_t offset,
                                       uint64_t size, bool copy_name,
                                       fbl::RefPtr<VmObject>* child_vmo) {
  LTRACEF("vmo %p offset %#" PRIx64 " size %#" PRIx64 "\n", this, offset, size);

  canary_.Assert();

  // Copy-on-write clones of contiguous VMOs do not have meaningful semantics, so forbid them.
  if (is_contiguous()) {
    return ZX_ERR_INVALID_ARGS;
  }

  // offset must be page aligned
  if (!IsPageRounded(offset)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // size must be page aligned and not too large.
  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (size > MAX_SIZE) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  auto cow_range = GetCowRange(offset, size);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  fbl::RefPtr<VmObjectPaged> vmo;

  {
    VmCowPages::DeferredOps deferred(cow_pages_.get());
    Guard<CriticalMutex> guard{lock()};
    // check that we're not uncached in some way
    if (self_locked()->GetMappingCachePolicyLocked() != ARCH_MMU_FLAG_CACHED) {
      return ZX_ERR_BAD_STATE;
    }

    // If we are a slice we require a unidirection clone, as performing a bi-directional clone
    // through a slice does not yet have defined semantics.
    const bool require_unidirection = is_slice();
    auto result =
        cow_pages_locked()->CreateCloneLocked(type, require_unidirection, *cow_range, deferred);
    if (result.is_error()) {
      return result.error_value();
    }

    uint32_t options = 0;
    if (resizable == Resizability::Resizable) {
      options |= kResizable;
    }
    if (can_block_on_page_requests()) {
      options |= kCanBlockOnPageRequests;
    }
    fbl::AllocChecker ac;
    auto [child, child_lock] = (*result).take();
    vmo = fbl::AdoptRef<VmObjectPaged>(new (&ac) VmObjectPaged(options, ktl::move(child)));
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
    Guard<CriticalMutex> child_guard{AdoptLock, vmo->lock(), ktl::move(child_lock)};
    DEBUG_ASSERT(vmo->self_locked()->GetMappingCachePolicyLocked() == ARCH_MMU_FLAG_CACHED);

    // Now that everything has succeeded we can wire up cow pages references. VMO will be placed in
    // the global list later once lock has been dropped.
    vmo->cow_pages_locked()->set_paged_backlink_locked(vmo.get());
    vmo->cow_pages_locked()->TransitionToAliveLocked();

    // Install the parent.
    {
      Guard<CriticalMutex> list_guard{ChildListLock::Get()};
      vmo->parent_ = this;

      // add the new vmo as a child before we do anything, since its
      // dtor expects to find it in its parent's child list
      AddChildLocked(vmo.get());
    }

    if (copy_name) {
      // There's no way good way to convince the static analysis that the vmo->lock() that we hold
      // is also the VmObject::lock() in vmo and so we disable analysis to set the name;
      [&vmo, &name = name_]() TA_REQ(vmo->lock()) TA_REQ(lock())
          TA_NO_THREAD_SAFETY_ANALYSIS { vmo->name_ = name; }();
    }
  }

  // Add to the global list now that fully initialized.
  vmo->AddToGlobalList();

  *child_vmo = ktl::move(vmo);

  return ZX_OK;
}

void VmObjectPaged::DumpLocked(uint depth, bool verbose) const {
  canary_.Assert();

  uint64_t parent_id = 0;
  // Cache the parent value as a void* as it's not safe to dereference once the ChildListLock is
  // dropped, but we can still print out its value.
  void* parent;
  {
    Guard<CriticalMutex> guard{ChildListLock::Get()};
    parent = parent_;
    if (parent_) {
      parent_id = parent_->user_id();
    }
  }

  for (uint i = 0; i < depth; ++i) {
    printf("  ");
  }
  printf("vmo %p/k%" PRIu64 " ref %d parent %p/k%" PRIu64 "\n", this, user_id_.load(),
         ref_count_debug(), parent, parent_id);

  char name[ZX_MAX_NAME_LEN];
  self_locked()->get_name_locked(name, sizeof(name));
  if (strlen(name) > 0) {
    for (uint i = 0; i < depth + 1; ++i) {
      printf("  ");
    }
    printf("name %s\n", name);
  }

  cow_pages_locked()->DumpLocked(depth, verbose);
}

VmObject::AttributionCounts VmObjectPaged::GetAttributedMemoryInRangeLocked(
    uint64_t offset_bytes, uint64_t len_bytes) const {
  vmo_attribution_queries.Add(1);

  // A reference never has memory attributed to it. It points to the parent's VmCowPages, and we
  // need to hold the invariant that we don't double-count attributed memory.
  //
  // TODO(https://fxbug.dev/42069078): Consider attributing memory to the current VmCowPages
  // backlink for the case where the parent has gone away.
  if (is_reference()) {
    return AttributionCounts{};
  }
  ASSERT(cow_range_.offset == 0);
  uint64_t new_len_bytes;
  if (!TrimRange(offset_bytes, len_bytes, size_locked(), &new_len_bytes)) {
    return AttributionCounts{};
  }

  auto cow_range = GetCowRange(offset_bytes, new_len_bytes);
  return cow_pages_locked()->GetAttributedMemoryInRangeLocked(*cow_range);
}

zx_status_t VmObjectPaged::CommitRangeInternal(uint64_t offset, uint64_t len, bool pin,
                                               bool write) {
  canary_.Assert();
  LTRACEF("offset %#" PRIx64 ", len %#" PRIx64 "\n", offset, len);

  if (can_block_on_page_requests()) {
    lockdep::AssertNoLocksHeld();
  }

  // We only expect write to be set if this a pin. All non-pin commits are reads.
  DEBUG_ASSERT(!write || pin);

  // Child slices of VMOs are currently not resizable, nor can they be made
  // from resizable parents.  If this ever changes, the logic surrounding what
  // to do if a VMO gets resized during a Commit or Pin operation will need to
  // be revisited.  Right now, we can just rely on the fact that the initial
  // vetting/trimming of the offset and length of the operation will never
  // change if the operation is being executed against a child slice.
  DEBUG_ASSERT(!is_resizable() || !is_slice());

  // Round offset and len to be page aligned. Use a sub-scope to validate that temporary end
  // calculations cannot be accidentally used later on.
  {
    uint64_t end;
    if (add_overflow(offset, len, &end)) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    const uint64_t end_page = RoundUpPageSize(end);
    if (end_page < end) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    DEBUG_ASSERT(end_page >= offset);
    offset = RoundDownPageSize(offset);
    len = end_page - offset;
  }

  // Although the length, for the non-pin ranges, is allowed to end up outside the VMO range during
  // the operation, at least initially it must be within range.
  {
    Guard<CriticalMutex> guard{lock()};
    if (unlikely(!InRange(offset, len, size_locked()))) {
      return ZX_ERR_OUT_OF_RANGE;
    }
  }

  if (len == 0) {
    // If pinning we explicitly forbid zero length pins as we cannot guarantee consistent semantics.
    // For example pinning a zero length range outside the range of the VMO is an error, and so
    // pinning a zero length range inside the vmo and then resizing the VMO smaller than the pin
    // region should also be an error. To enforce this without having to have new metadata to track
    // zero length pin regions is to just forbid them. Note that the user entry points for pinning
    // already forbid zero length ranges.
    return pin ? ZX_ERR_INVALID_ARGS : ZX_OK;
  }

  // Tracks the end of the pinned range to unpin in case of failure. The |offset| might lag behind
  // the pinned range, as it tracks the range that has been completely processed, which would
  // also include dirtying the page after pinning in case of a write.
  uint64_t pinned_end_offset = offset;
  // Should any errors occur we need to unpin everything. If we were asked to write, we need to mark
  // the VMO modified if any pages were committed.
  auto deferred_cleanup =
      fit::defer([this, pinned_start_offset = offset, &pinned_end_offset, &len, &write]() {
        // If we were not able to pin the entire range, i.e. len is not 0, we need to unpin
        // everything. Regardless of any resizes or other things that may have happened any pinned
        // pages *must* still be within a valid range, and so we know Unpin should succeed. The edge
        // case is if we had failed to pin *any* pages and so our original offset may be outside the
        // current range of the vmo. Additionally, as pinning a zero length range is invalid, so is
        // unpinning, and so we must avoid.
        if (pinned_end_offset > pinned_start_offset) {
          if (len > 0) {
            auto cow_range =
                GetCowRange(pinned_start_offset, pinned_end_offset - pinned_start_offset);
            Guard<CriticalMutex> guard{AssertOrderedLock, lock(), cow_pages_->lock_order()};
            cow_pages_locked()->UnpinLocked(*cow_range, nullptr);
          } else if (write) {
            Guard<CriticalMutex> guard{AssertOrderedLock, lock(), cow_pages_->lock_order()};
            mark_modified_locked();
          }
        }
      });

  __UNINITIALIZED MultiPageRequest page_request;

  // As we may need to wait on arbitrary page requests we just keep running this as long as there is
  // a non-zero range to process.
  uint64_t to_dirty_len = 0;
  while (len > 0) {
    zx_status_t status = ZX_OK;
    uint64_t committed_len = 0;
    if (to_dirty_len > 0) {
      Guard<CriticalMutex> guard{AssertOrderedLock, lock(), cow_pages_->lock_order()};
      // The to_dirty_len *must* be within range, even though we just grabbed the lock and a resize
      // could have happened, since the dirtied range is pinned. As such, any resize could not have
      // removed the in progress dirty range.
      DEBUG_ASSERT(InRange(offset, to_dirty_len, size_locked()));
      uint64_t dirty_len = 0;
      status = cow_pages_locked()->PrepareForWriteLocked(
          *GetCowRange(offset, to_dirty_len), page_request.GetLazyDirtyRequest(), &dirty_len);
      DEBUG_ASSERT(dirty_len <= to_dirty_len);
      if (status == ZX_ERR_SHOULD_WAIT) {
        page_request.MadeDirtyRequest();
      }
      // Account for the pages that were dirtied during this attempt.
      to_dirty_len -= dirty_len;
      committed_len = dirty_len;
    } else {
      __UNINITIALIZED VmCowPages::DeferredOps deferred(cow_pages_.get());
      Guard<CriticalMutex> guard{AssertOrderedLock, lock(), cow_pages_->lock_order()};
      uint64_t new_len = len;
      if (!TrimRange(offset, len, size_locked(), &new_len)) {
        return pin ? ZX_ERR_OUT_OF_RANGE : ZX_OK;
      }
      if (new_len != len) {
        if (pin) {
          return ZX_ERR_OUT_OF_RANGE;
        }
        len = new_len;
        if (len == 0) {
          break;
        }
      }

      status = cow_pages_locked()->CommitRangeLocked(*GetCowRange(offset, len), deferred,
                                                     &committed_len, &page_request);
      DEBUG_ASSERT(committed_len <= len);

      // If we're required to pin, try to pin the committed range before waiting on the
      // page_request, which has been populated to request pages beyond the committed range. Even
      // though the page_request has already been initialized, we choose to first completely process
      // the committed range, which could end up canceling the already initialized page request.
      // This allows us to keep making forward progress as we will potentially pin a few pages
      // before trying to fault in further pages, thereby preventing the already committed (and
      // pinned) pages from being evicted while we wait with the lock dropped.
      if (pin && committed_len > 0) {
        uint64_t non_loaned_len = 0;
        if (cow_pages_locked()->can_borrow() &&
            PhysicalPageBorrowingConfig::Get().is_loaning_enabled()) {
          // We need to replace any loaned pages in the committed range with non-loaned pages first,
          // since pinning expects all pages to be non-loaned. Replacing loaned pages requires a
          // page request too. At any time we'll only be able to wait on a single page request, and
          // after the wait the conditions that resulted in the previous request might have changed,
          // so we can just cancel and reuse the existing page_request.
          // TODO: consider not canceling this and the other request below. The issue with not
          // canceling is that without early wake support, i.e. being able to reinitialize an
          // existing initialized request, I think this code will not work without canceling.
          page_request.CancelRequests();
          status = cow_pages_locked()->ReplacePagesWithNonLoanedLocked(
              *GetCowRange(offset, committed_len), deferred, page_request.GetAnonymous(),
              &non_loaned_len);
          DEBUG_ASSERT(non_loaned_len <= committed_len);
        } else {
          // Either the VMO does not support borrowing, or loaning is not enabled so we know there
          // are no loaned pages.
          non_loaned_len = committed_len;
        }

        // We can safely pin the non-loaned range before waiting on the page request.
        if (non_loaned_len > 0) {
          // Verify that we are starting the pin after the previously pinned range, as we do not
          // want to repeatedly pin the same pages.
          ASSERT(pinned_end_offset == offset);
          zx_status_t pin_status =
              cow_pages_locked()->PinRangeLocked(*GetCowRange(offset, non_loaned_len));
          if (pin_status != ZX_OK) {
            return pin_status;
          }
        }
        // At this point we have successfully committed and pinned non_loaned_len.
        uint64_t pinned_len = non_loaned_len;
        pinned_end_offset = offset + pinned_len;

        // If this is a write and the VMO supports dirty tracking, we also need to mark the pinned
        // pages Dirty.
        // We pin the pages first before marking them dirty in order to guarantee forward progress.
        // Pinning the pages will prevent them from getting decommitted while we are waiting on the
        // dirty page request without the lock held.
        if (write && pinned_len > 0 && is_dirty_tracked()) {
          // Prepare the committed range for writing. We need a page request for this too, so cancel
          // any existing one and reuse it.
          page_request.CancelRequests();

          // We want to dirty the entire pinned range.
          to_dirty_len = pinned_len;
          continue;
        }
        committed_len = pinned_len;
      }
    }
    if (status == ZX_ERR_SHOULD_WAIT) {
      status = page_request.Wait();
    }
    if (status != ZX_OK) {
      if (status == ZX_ERR_TIMED_OUT) {
        Dump(0, false);
      }
      return status;
    }
    offset += committed_len;
    len -= committed_len;
  }
  return ZX_OK;
}

zx_status_t VmObjectPaged::DecommitRange(uint64_t offset, uint64_t len) {
  canary_.Assert();
  LTRACEF("offset %#" PRIx64 ", len %#" PRIx64 "\n", offset, len);

  if (is_contiguous() && !PhysicalPageBorrowingConfig::Get().is_loaning_enabled()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // Decommit of pages from a contiguous VMO relies on contiguous VMOs not being resizable.
  DEBUG_ASSERT(!is_resizable() || !is_contiguous());

  return cow_pages_->DecommitRange(*cow_range);
}

zx_status_t VmObjectPaged::ZeroPartialPage(uint64_t page_base_offset, uint64_t zero_start_offset,
                                           uint64_t zero_end_offset) {
  DEBUG_ASSERT(zero_start_offset <= zero_end_offset);
  DEBUG_ASSERT(zero_end_offset <= kPageSize);
  DEBUG_ASSERT(IsPageRounded(page_base_offset));

  {
    Guard<CriticalMutex> guard{lock()};

    if (page_base_offset >= size_locked()) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    // TODO: Consider replacing this with a more appropriate generic API when one is available.
    if (cow_pages_locked()->PageWouldReadZeroLocked(page_base_offset)) {
      // This is already considered zero so no need to redundantly zero again.
      return ZX_OK;
    }
  }

  // Need to actually zero out bytes in the page.
  return ReadWriteInternal(page_base_offset + zero_start_offset,
                           zero_end_offset - zero_start_offset, true,
                           VmObjectReadWriteOptions::None,
                           [](void* dst, size_t offset, size_t len) -> UserCopyCaptureFaultsResult {
                             // We're memsetting the *kernel* address of an allocated page, so we
                             // know that this cannot fault. memset may not be the most efficient,
                             // but we don't expect to be doing this very often.
                             memset(dst, 0, len);
                             return UserCopyCaptureFaultsResult{ZX_OK};
                           })
      .first;
}

zx_status_t VmObjectPaged::ZeroRangeInternal(uint64_t offset, uint64_t len, bool dirty_track) {
  canary_.Assert();
  if (can_block_on_page_requests()) {
    lockdep::AssertNoLocksHeld();
  }
  // May need to zero in chunks across multiple different lock acquisitions so loop until nothing
  // left to do.
  while (len > 0) {
    // Check for any non-page aligned start and handle separately.
    if (!IsPageRounded(offset)) {
      // We're doing partial page writes, so we should be dirty tracking.
      DEBUG_ASSERT(dirty_track);
      const uint64_t page_base = RoundDownPageSize(offset);
      const uint64_t zero_start_offset = offset - page_base;
      const uint64_t zero_len = ktl::min(kPageSize - zero_start_offset, len);
      zx_status_t status =
          ZeroPartialPage(page_base, zero_start_offset, zero_start_offset + zero_len);
      if (status != ZX_OK) {
        return status;
      }
      // Advance over the length we zeroed and then, since the lock might have been dropped, go
      // around the loop to redo the checks.
      offset += zero_len;
      len -= zero_len;
      continue;
    }
    // The start is page aligned, so if the remaining length is not a page size then perform the
    // final sub-page zero.
    if (len < kPageSize) {
      DEBUG_ASSERT(dirty_track);
      return ZeroPartialPage(offset, 0, len);
    }

    // First try and do the more efficient decommit. We prefer/ decommit as it performs work in the
    // order of the number of committed pages, instead of work in the order of size of the range. An
    // error from DecommitRangeLocked indicates that the VMO is not of a form that decommit can
    // safely be performed without exposing data that we shouldn't between children and parents, but
    // no actual state will have been changed. Should decommit succeed we are done, otherwise we
    // will have to handle each offset individually.
    //
    // Zeroing doesn't decommit pages of contiguous VMOs.
    if (!is_contiguous()) {
      ktl::optional<VmCowRange> cow_range = GetCowRange(offset, RoundDownPageSize(len));
      if (!cow_range) {
        return ZX_ERR_OUT_OF_RANGE;
      }

      zx_status_t status = cow_pages_->DecommitRange(*cow_range);
      if (status == ZX_OK) {
        offset += cow_range->len;
        len -= cow_range->len;
        continue;
      }
    }

    // We might need a page request if the VMO is backed by a page source.
    __UNINITIALIZED MultiPageRequest page_request;
    uint64_t zeroed_len;
    zx_status_t status;
    {
      __UNINITIALIZED VmCowPages::DeferredOps deferred(cow_pages_.get());
      Guard<CriticalMutex> guard{lock()};

      // Zeroing a range behaves as if it were an efficient zx_vmo_write. As we cannot write to
      // uncached vmo, we also cannot zero an uncahced vmo.
      if (self_locked()->GetMappingCachePolicyLocked() != ARCH_MMU_FLAG_CACHED) {
        return ZX_ERR_BAD_STATE;
      }

      // Offset is page aligned, and we have at least one full page to process, so find the page
      // aligned length to hand over to the cow pages zero method.
      ktl::optional<VmCowRange> cow_range =
          GetCowRangeSizeCheckLocked(offset, RoundDownPageSize(len));
      if (!cow_range) {
        return ZX_ERR_OUT_OF_RANGE;
      }

#if DEBUG_ASSERT_IMPLEMENTED
      // Currently we want ZeroPagesLocked() to not decommit any pages from a contiguous VMO.  In
      // debug we can assert that (not a super fast assert, but seems worthwhile; it's debug only).
      uint64_t page_count_before =
          is_contiguous() ? cow_pages_locked()->DebugGetPageCountLocked() : 0;
#endif
      // Now that we have a page aligned range we can try hand over to the cow pages zero method.
      ktl::tie(status, zeroed_len) =
          cow_pages_locked()->ZeroPagesLocked(*cow_range, dirty_track, deferred, &page_request);
      if (zeroed_len != 0) {
        // Mark modified since we wrote zeros.
        mark_modified_locked();
      }

#if DEBUG_ASSERT_IMPLEMENTED
      if (is_contiguous()) {
        uint64_t page_count_after = cow_pages_locked()->DebugGetPageCountLocked();
        DEBUG_ASSERT(page_count_after == page_count_before);
      }
#endif
    }

    // Wait on any page request, which is the only non-fatal error case.
    if (status == ZX_ERR_SHOULD_WAIT) {
      status = page_request.Wait();
      if (status == ZX_ERR_TIMED_OUT) {
        Dump(0, false);
      }
    }
    if (status != ZX_OK) {
      return status;
    }
    // Advance over pages that had already been zeroed.
    offset += zeroed_len;
    len -= zeroed_len;
  }
  return ZX_OK;
}

zx_status_t VmObjectPaged::Resize(uint64_t s) {
  canary_.Assert();

  LTRACEF("vmo %p, size %" PRIu64 "\n", this, s);

  DEBUG_ASSERT(!is_contiguous() || !is_resizable());
  // Also rejects contiguous VMOs.
  if (!is_resizable()) {
    return ZX_ERR_UNAVAILABLE;
  }

  // ensure the size is valid and that we will not wrap.
  if (!IsPageRounded(s)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (s > MAX_SIZE) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  return cow_pages_->Resize(s);
}

// perform some sort of copy in/out on a range of the object using a passed in lambda for the copy
// routine. The copy routine has the expected type signature of: (void *ptr, uint64_t offset,
//  uint64_t len) -> UserCopyCaptureFaultsResult.
template <typename T>
ktl::pair<zx_status_t, size_t> VmObjectPaged::ReadWriteInternal(uint64_t offset, size_t len,
                                                                bool write,
                                                                VmObjectReadWriteOptions options,
                                                                T copyfunc) {
  canary_.Assert();

  uint64_t end_offset;
  if (add_overflow(offset, len, &end_offset)) {
    return {ZX_ERR_OUT_OF_RANGE, 0};
  }

  // Track our two offsets.
  uint64_t src_offset = offset;
  size_t dest_offset = 0;

  // The PageRequest is a non-trivial object so we declare it outside the loop to avoid having to
  // construct and deconstruct it each iteration. It is tolerant of being reused and will
  // reinitialize itself if needed.
  // Ideally we can wake up early from the page request to begin processing any partially supplied
  // ranges. However, if performing a write to a dirty tracked VMO this is not presently possible as
  // we need to first read in the range and then dirty it, and we cannot have both a read and dirty
  // request outstanding at one time.
  __UNINITIALIZED MultiPageRequest page_request(!write);
  do {
    zx_status_t status;
    __UNINITIALIZED UserCopyCaptureFaultsResult copy_result(ZX_OK);
    {
      __UNINITIALIZED VmCowPages::DeferredOps deferred(cow_pages_.get());
      Guard<CriticalMutex> guard{AssertOrderedLock, lock(), cow_pages_->lock_order()};
      if (self_locked()->GetMappingCachePolicyLocked() != ARCH_MMU_FLAG_CACHED) {
        return {ZX_ERR_BAD_STATE, src_offset - offset};
      }
      if (end_offset > size_locked()) {
        if (!!(options & VmObjectReadWriteOptions::TrimLength)) {
          if (src_offset >= size_locked()) {
            return {ZX_OK, src_offset - offset};
          }
          end_offset = size_locked();
        } else {
          return {ZX_ERR_OUT_OF_RANGE, src_offset - offset};
        }
      } else if (src_offset >= end_offset) {
        return {ZX_OK, src_offset - offset};
      }

      const size_t first_page_offset = RoundDownPageSize(src_offset);
      const size_t last_page_offset = RoundDownPageSize(end_offset - 1);
      size_t remaining_pages = (last_page_offset - first_page_offset) / kPageSize + 1;
      size_t pages_since_last_unlock = 0;
      bool modified = false;

      __UNINITIALIZED zx::result<VmCowPages::LookupCursor> cursor =
          GetLookupCursorLocked(first_page_offset, remaining_pages * kPageSize);
      if (cursor.is_error()) {
        return {cursor.status_value(), src_offset - offset};
      }
      // Performing explicit accesses by request of the user, so disable zero forking.
      cursor->DisableZeroFork();
      AssertHeld(cursor->lock_ref());

      while (remaining_pages > 0) {
        const size_t page_offset = src_offset % kPageSize;
        const size_t tocopy = ktl::min(kPageSize - page_offset, end_offset - src_offset);

        // If we need to wait on pages then we would like to wait on as many as possible, up to the
        // actual limit of the read/write operation. For a read we can wake up once some pages are
        // received, minimizing the latency before we start making progress, but as this is not true
        // for writes we cap the maximum number requested.
        constexpr uint64_t kMaxWriteWaitPages = 256;  // 1MB batch size (256 * 4KB pages)
        const uint64_t max_wait_pages = write ? kMaxWriteWaitPages : UINT64_MAX;
        const uint64_t max_waitable_pages = ktl::min(remaining_pages, max_wait_pages);

        // Attempt to lookup a page
        __UNINITIALIZED zx::result<VmCowPages::LookupCursor::RequireResult> result =
            cursor->RequirePage(write, max_waitable_pages, deferred, &page_request);

        status = result.status_value();
        if (status != ZX_OK) {
          break;
        }

        // Compute the kernel mapping of this page.
        const paddr_t pa = result->page->paddr();
        char* page_ptr = reinterpret_cast<char*>(paddr_to_physmap(pa));

        // Call the copy routine. If the copy was successful then ZX_OK is returned, otherwise
        // ZX_ERR_SHOULD_WAIT may be returned to indicate the copy failed but we can retry it.
        copy_result = copyfunc(page_ptr + page_offset, dest_offset, tocopy);

        // If a fault has actually occurred, then we will have captured fault info that we can use
        // to handle the fault.
        if (copy_result.fault_info.has_value()) {
          break;
        }
        // If we encounter _any_ unrecoverable error from the copy operation which
        // produced no fault address, squash the error down to just "NOT_FOUND".
        // This is what the SoftFault error would have told us if we did try to
        // handle the fault and could not.
        if (copy_result.status != ZX_OK) {
          status = ZX_ERR_NOT_FOUND;
          break;
        }
        // Advance the copy location.
        src_offset += tocopy;
        dest_offset += tocopy;
        remaining_pages--;
        modified = write;

        // Periodically yield the lock in order to allow other read or write
        // operations to advance sooner than they otherwise would.
        constexpr size_t kPagesBetweenUnlocks = 16;
        if (unlikely(++pages_since_last_unlock == kPagesBetweenUnlocks)) {
          pages_since_last_unlock = 0;
          if (guard.lock()->IsContested()) {
            break;
          }
        }
      }
      // Before dropping the lock, check if any pages were modified and update the VMO state
      // accordingly.
      if (modified) {
        mark_modified_locked();
      }
    }

    // If there was a fault while copying, then handle it now that the lock is dropped.
    if (copy_result.fault_info.has_value()) {
      auto& info = *copy_result.fault_info;
      uint64_t to_fault = len - dest_offset;
      status = Thread::Current::SoftFaultInRange(info.pf_va, info.pf_flags, to_fault);
    } else if (status == ZX_ERR_SHOULD_WAIT) {
      // RequirePage 'failed', but told us that it had filled out the page request, so we should
      // wait on it.
      DEBUG_ASSERT(can_block_on_page_requests());
      status = page_request.Wait();
      if (status == ZX_ERR_TIMED_OUT) {
        Dump(0, false);
      }
    }
    if (status != ZX_OK) {
      return {status, src_offset - offset};
    }
  } while (src_offset < end_offset);

  return {ZX_OK, src_offset - offset};
}

zx_status_t VmObjectPaged::Read(void* _ptr, uint64_t offset, size_t len) {
  canary_.Assert();
  // test to make sure this is a kernel pointer
  if (!is_kernel_address(reinterpret_cast<vaddr_t>(_ptr))) {
    DEBUG_ASSERT_MSG(0, "non kernel pointer passed\n");
    return ZX_ERR_INVALID_ARGS;
  }

  // read routine that just uses a memcpy
  char* ptr = reinterpret_cast<char*>(_ptr);
  auto read_routine = [ptr](const void* src, size_t offset,
                            size_t len) -> UserCopyCaptureFaultsResult {
    memcpy(ptr + offset, src, len);
    return UserCopyCaptureFaultsResult{ZX_OK};
  };

  if (can_block_on_page_requests()) {
    lockdep::AssertNoLocksHeld();
  }

  return ReadWriteInternal(offset, len, false, VmObjectReadWriteOptions::None, read_routine).first;
}

zx_status_t VmObjectPaged::Write(const void* _ptr, uint64_t offset, size_t len) {
  canary_.Assert();
  // test to make sure this is a kernel pointer
  if (!is_kernel_address(reinterpret_cast<vaddr_t>(_ptr))) {
    DEBUG_ASSERT_MSG(0, "non kernel pointer passed\n");
    return ZX_ERR_INVALID_ARGS;
  }

  // write routine that just uses a memcpy
  const char* ptr = reinterpret_cast<const char*>(_ptr);
  auto write_routine = [ptr](void* dst, size_t offset, size_t len) -> UserCopyCaptureFaultsResult {
    memcpy(dst, ptr + offset, len);
    return UserCopyCaptureFaultsResult{ZX_OK};
  };

  if (can_block_on_page_requests()) {
    lockdep::AssertNoLocksHeld();
  }

  return ReadWriteInternal(offset, len, true, VmObjectReadWriteOptions::None, write_routine).first;
}

zx_status_t VmObjectPaged::CacheOp(uint64_t offset, uint64_t len, CacheOpType type) {
  canary_.Assert();
  if (unlikely(len == 0)) {
    return ZX_ERR_INVALID_ARGS;
  }

  Guard<CriticalMutex> guard{lock()};

  // verify that the range is within the object
  auto cow_range = GetCowRangeSizeCheckLocked(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // This cannot overflow as we already checked the range.
  const uint64_t cow_end = cow_range->end();

  // For syncing instruction caches there may be work that is more efficient to batch together, and
  // so we use an abstract consistency manager to optimize it for the given architecture.
  ArchVmICacheConsistencyManager sync_cm;

  return cow_pages_locked()->LookupReadableLocked(
      *cow_range,
      [&sync_cm, cow_offset = cow_range->offset, cow_end, type](uint64_t page_offset, paddr_t pa) {
        // This cannot overflow due to the maximum possible size of a VMO.
        const uint64_t page_end = page_offset + kPageSize;

        // Determine our start and end in terms of vmo offset
        const uint64_t start = ktl::max(page_offset, cow_offset);
        const uint64_t end = ktl::min(cow_end, page_end);

        // Translate to inter-page offset
        DEBUG_ASSERT(start >= page_offset);
        const uint64_t op_start_offset = start - page_offset;
        DEBUG_ASSERT(op_start_offset < kPageSize);

        DEBUG_ASSERT(end > start);
        const uint64_t op_len = end - start;

        CacheOpPhys(pa + op_start_offset, op_len, type, sync_cm);
        return ZX_ERR_NEXT;
      });
}

zx_status_t VmObjectPaged::Lookup(uint64_t offset, uint64_t len,
                                  VmObject::LookupFunction lookup_fn) {
  canary_.Assert();
  VmCowRange range(offset, len);
  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  Guard<CriticalMutex> guard{lock()};

  return cow_pages_locked()->LookupLocked(
      *cow_range, [&lookup_fn, undo_offset = cow_range_.offset](uint64_t offset, paddr_t pa) {
        // Need to undo the parent_offset before forwarding to the lookup_fn, who is ignorant of
        // slices.
        return lookup_fn(offset - undo_offset, pa);
      });
}

zx_status_t VmObjectPaged::LookupContiguous(uint64_t offset, uint64_t len, paddr_t* out_paddr) {
  canary_.Assert();

  // We should consider having the callers round up to page boundaries and then check whether the
  // length is page-aligned.
  if (unlikely(len == 0 || !IsPageRounded(offset))) {
    return ZX_ERR_INVALID_ARGS;
  }

  Guard<CriticalMutex> guard{lock()};

  auto cow_range = GetCowRangeSizeCheckLocked(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  if (unlikely(!is_contiguous() && (cow_range->len != kPageSize))) {
    // Multi-page lookup only supported for contiguous VMOs.
    return ZX_ERR_BAD_STATE;
  }

  // Verify that all pages are present, and assert that the present pages are contiguous since we
  // only support len > kPageSize for contiguous VMOs.
  bool page_seen = false;
  uint64_t first_offset = 0;
  paddr_t first_paddr = 0;
  uint64_t count = 0;
  // This has to work for child slices with non-zero cow_range_.offset also, which means even if all
  // pages are present, the first cur_offset can be offset + cow_range_.offset.
  zx_status_t status = cow_pages_locked()->LookupLocked(
      *cow_range,
      [&page_seen, &first_offset, &first_paddr, &count](uint64_t cur_offset, paddr_t pa) mutable {
        ++count;
        if (!page_seen) {
          first_offset = cur_offset;
          first_paddr = pa;
          page_seen = true;
        }
        ASSERT(first_paddr + (cur_offset - first_offset) == pa);
        return ZX_ERR_NEXT;
      });
  ASSERT(status == ZX_OK);
  if (count != cow_range->len / kPageSize) {
    return ZX_ERR_NOT_FOUND;
  }
  if (out_paddr) {
    *out_paddr = first_paddr;
  }
  return ZX_OK;
}

ktl::pair<zx_status_t, size_t> VmObjectPaged::ReadUser(user_out_ptr<char> ptr, uint64_t offset,
                                                       size_t len,
                                                       VmObjectReadWriteOptions options) {
  canary_.Assert();

  // read routine that uses copy_to_user
  auto read_routine = [ptr](const char* src, size_t offset,
                            size_t len) -> UserCopyCaptureFaultsResult {
    return ptr.byte_offset(offset).copy_array_to_user_capture_faults(src, len);
  };

  // As we may need to perform fault resolution later, ensure our caller is not holding any locks.
  lockdep::AssertNoLocksHeld();

  return ReadWriteInternal(offset, len, false, options, read_routine);
}

ktl::pair<zx_status_t, size_t> VmObjectPaged::WriteUser(
    user_in_ptr<const char> ptr, uint64_t offset, size_t len, VmObjectReadWriteOptions options,
    const OnWriteBytesTransferredCallback& on_bytes_transferred) {
  canary_.Assert();

  // write routine that uses copy_from_user
  auto write_routine = [ptr, base_vmo_offset = offset, &on_bytes_transferred](
                           char* dst, size_t offset, size_t len) -> UserCopyCaptureFaultsResult {
    __UNINITIALIZED auto copy_result =
        ptr.byte_offset(offset).copy_array_from_user_capture_faults(dst, len);

    if (copy_result.status == ZX_OK) {
      if (on_bytes_transferred) {
        on_bytes_transferred(base_vmo_offset + offset, len);
      }
    }
    return copy_result;
  };

  // As we may need to perform fault resolution later, ensure our caller is not holding any locks.
  lockdep::AssertNoLocksHeld();

  return ReadWriteInternal(offset, len, true, options, write_routine);
}

ktl::pair<zx_status_t, size_t> VmObjectPaged::ReadUserVector(user_out_iovec_t vec, uint64_t offset,
                                                             size_t len) {
  if (len == 0u) {
    return {ZX_OK, 0};
  }
  if (len > UINT64_MAX - offset) {
    return {ZX_ERR_OUT_OF_RANGE, 0};
  }

  size_t total = 0;
  zx_status_t status = vec.ForEach([&](user_out_ptr<char> ptr, size_t capacity) {
    if (capacity > len) {
      capacity = len;
    }

    auto [read_status, chunk_actual] =
        ReadUser(ptr, offset, capacity, VmObjectReadWriteOptions::None);

    // Always add |chunk_actual| since some bytes may have been transferred, even on error
    total += chunk_actual;
    if (read_status != ZX_OK) {
      return read_status;
    }

    DEBUG_ASSERT(chunk_actual == capacity);

    offset += chunk_actual;
    len -= chunk_actual;
    return len > 0 ? ZX_ERR_NEXT : ZX_ERR_STOP;
  });

  // Return |ZX_ERR_BUFFER_TOO_SMALL| if all of |len| was not transferred.
  status = (status == ZX_OK && len > 0) ? ZX_ERR_BUFFER_TOO_SMALL : status;
  return {status, total};
}

ktl::pair<zx_status_t, size_t> VmObjectPaged::WriteUserVector(
    user_in_iovec_t vec, uint64_t offset, size_t len,
    const OnWriteBytesTransferredCallback& on_bytes_transferred) {
  if (len == 0u) {
    return {ZX_OK, 0};
  }
  if (len > UINT64_MAX - offset) {
    return {ZX_ERR_OUT_OF_RANGE, 0};
  }

  size_t total = 0;
  zx_status_t status = vec.ForEach([&](user_in_ptr<const char> ptr, size_t capacity) {
    if (capacity > len) {
      capacity = len;
    }

    auto [write_status, chunk_actual] =
        WriteUser(ptr, offset, capacity, VmObjectReadWriteOptions::None, on_bytes_transferred);

    // Always add |chunk_actual| since some bytes may have been transferred, even on error
    total += chunk_actual;
    if (write_status != ZX_OK) {
      return write_status;
    }

    DEBUG_ASSERT(chunk_actual == capacity);

    offset += chunk_actual;
    len -= chunk_actual;
    return len > 0 ? ZX_ERR_NEXT : ZX_ERR_STOP;
  });

  // Return |ZX_ERR_BUFFER_TOO_SMALL| if all of |len| was not transferred.
  status = (status == ZX_OK && len > 0) ? ZX_ERR_BUFFER_TOO_SMALL : status;
  return {status, total};
}

zx_status_t VmObjectPaged::TakePages(uint64_t offset, uint64_t len, VmPageSpliceList* pages) {
  canary_.Assert();

  // TODO: Check that the region is locked once locking is implemented
  if (is_contiguous()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  auto range = *cow_range;

  // Initialize the splice list to the right size.
  pages->Initialize(range.len);
  uint64_t splice_offset = 0;

  __UNINITIALIZED MultiPageRequest page_request;
  while (!range.is_empty()) {
    uint64_t taken_len = 0;
    zx_status_t status =
        cow_pages_->TakePages(range, splice_offset, pages, &taken_len, &page_request);
    if (status != ZX_ERR_SHOULD_WAIT && status != ZX_OK) {
      return status;
    }
    // We would only have failed to take anything if status was not ZX_OK, which in this case
    // would be ZX_ERR_SHOULD_WAIT as that is the only non-OK status we can reach here with.
    DEBUG_ASSERT(taken_len > 0 || status == ZX_ERR_SHOULD_WAIT);
    // We should have taken the entire range requested if the status was ZX_OK.
    DEBUG_ASSERT(status != ZX_OK || taken_len == range.len);
    // We should not have taken any more than the requested range.
    DEBUG_ASSERT(taken_len <= range.len);

    splice_offset += taken_len;

    // Record the completed portion.
    range = range.TrimmedFromStart(taken_len);

    if (status == ZX_ERR_SHOULD_WAIT) {
      status = page_request.Wait();
      if (status != ZX_OK) {
        return status;
      }
    }
  }
  return ZX_OK;
}

zx_status_t VmObjectPaged::SupplyPages(uint64_t offset, uint64_t len, VmPageSpliceList* pages,
                                       SupplyOptions options) {
  canary_.Assert();

  // We need this check here instead of in SupplyPagesLocked, as we do use that
  // function to provide pages to contiguous VMOs as well.
  if (is_contiguous()) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  auto range = *cow_range;

  if (range.is_empty()) {
    return ZX_OK;
  }

  // Pre-process pages, which includes converting any references to real pages if necessary. Forward
  // progress is guaranteed as pages in the VmSpliceList aren't tracked in page queues, and can't be
  // compressed back into references.
  zx_status_t status = cow_pages_->ProcessPagesForSupply(pages);
  if (status != ZX_OK) {
    return status;
  }

  __UNINITIALIZED MultiPageRequest page_request;
  {
    __UNINITIALIZED VmCowPages::DeferredOps deferred(cow_pages_.get());
    Guard<CriticalMutex> guard{lock()};
    status = cow_pages_locked()->SupplyPagesLocked(range, pages, options, deferred, &page_request);
  }

  return status;
}

zx_status_t VmObjectPaged::DirtyPages(uint64_t offset, uint64_t len) {
  // It is possible to encounter delayed PMM allocations, which requires waiting on the
  // page_request.
  __UNINITIALIZED AnonymousPageRequest page_request;

  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // Initialize a list of allocated pages that DirtyPages will allocate any new pages into
  // before inserting them in the VMO. Allocated pages can therefore be shared across multiple calls
  // to DirtyPages. Instead of having to allocate and free pages in case DirtyPages
  // cannot successfully dirty the entire range atomically, we can just hold on to the allocated
  // pages and use them for the next call. This ensures that we are making forward progress with
  // each successive call to DirtyPages.
  VmPageDoublyLinkedList alloc_list;
  auto alloc_list_cleanup = fit::defer([&alloc_list, this]() -> void {
    if (!alloc_list.is_empty()) {
      cow_pages_->FreePages(&alloc_list);
    }
  });
  while (true) {
    zx_status_t status = cow_pages_->DirtyPages(*cow_range, &alloc_list, &page_request);
    if (status == ZX_OK) {
      return ZX_OK;
    }
    if (status == ZX_ERR_SHOULD_WAIT) {
      status = page_request.Allocate().status_value();
    }
    if (status != ZX_OK) {
      return status;
    }
    // If the wait was successful, loop around and try the call again, which will re-validate any
    // state that might have changed when the lock was dropped.
  }
}

zx_status_t VmObjectPaged::EnumerateDirtyRanges(uint64_t offset, uint64_t len,
                                                DirtyRangeEnumerateFunction&& dirty_range_fn) {
  Guard<CriticalMutex> guard{lock()};
  if (auto cow_range = GetCowRange(offset, len)) {
    // Need to wrap the callback to translate the cow pages offsets back into offsets as seen by
    // this object.
    return cow_pages_locked()->EnumerateDirtyRangesLocked(
        *cow_range, [&dirty_range_fn, undo_offset = cow_range_.offset](
                        uint64_t range_offset, uint64_t range_len, bool range_is_zero) {
          return dirty_range_fn(range_offset - undo_offset, range_len, range_is_zero);
        });
  }
  return ZX_ERR_OUT_OF_RANGE;
}

zx_status_t VmObjectPaged::SetMappingCachePolicy(const arch_mmu_flags_t cache_policy) {
  // Is it a valid cache flag?
  if (cache_policy & ~ZX_CACHE_POLICY_MASK) {
    return ZX_ERR_INVALID_ARGS;
  }

  Guard<CriticalMutex> guard{lock()};

  // conditions for allowing the cache policy to be set:
  // 1) vmo has no pinned pages
  // 2) vmo has no mappings
  // 3) vmo has no children
  // 4) vmo is not a child
  if (cow_pages_locked()->pinned_page_count_locked() > 0) {
    return ZX_ERR_BAD_STATE;
  }

  if (self_locked()->num_mappings_locked() != 0) {
    return ZX_ERR_BAD_STATE;
  }

  // The ChildListLock needs to be held to inspect the children/parent pointers, however we do not
  // need to hold it over the remainder of this method as the main VMO lock is held, and creating a
  // new child happens under that lock as well since the creation path must, in a single lock
  // acquisition, be checking the cache_policy_ and creating the child.
  {
    Guard<CriticalMutex> child_guard{ChildListLock::Get()};

    if (!children_list_.is_empty()) {
      return ZX_ERR_BAD_STATE;
    }
    if (parent_) {
      return ZX_ERR_BAD_STATE;
    }
  }

  // Forbid if there are references, or if this object is a reference itself. We do not want cache
  // policies to diverge across references. Note that this check is required in addition to the
  // children_list_ and parent_ check, because it is possible for a non-reference parent to go away,
  // which will trigger the election of a reference as the new owner for the remaining
  // reference_list_, and also reset the parent_.
  if (!reference_list_.is_empty()) {
    return ZX_ERR_BAD_STATE;
  }
  if (is_reference()) {
    return ZX_ERR_BAD_STATE;
  }

  // It does not make sense for a pager-backed or discardable VMO to be uncached.
  if (is_user_pager_backed() || is_discardable()) {
    DEBUG_ASSERT(GetMappingCachePolicyLocked() == ARCH_MMU_FLAG_CACHED);
    return cache_policy == ARCH_MMU_FLAG_CACHED ? ZX_OK : ZX_ERR_BAD_STATE;
  }

  // Set the cache policy before informing the VmCowPages, as it may make decisions based on the
  // final cache policy.
  // There's no way good way to convince the static analysis that the lock() that we hold is
  // also the VmObject::lock() and so we disable analysis to set the cache_policy_;
  [this, &cache_policy]() TA_REQ(lock())
      TA_NO_THREAD_SAFETY_ANALYSIS { cache_policy_ = cache_policy; }();

  // Asks the cow pages to perform any internal transitions and, most importantly, clean and
  // invalidate any committed pages. In the case of going from cached->uncached the clean+invalidate
  // ensures that any modifications are cleaned back to RAM so that an uncached mapping sees any
  // modifications made prior to changing the cache policy. When going from uncached->cached due to
  // the cached physmap there could be cache lines that hold stale data of the pages that were
  // modified via an uncached mapping. As these cache lines are, by definition, clean, a
  // clean+invalidate will simply invalidate them and not write them back, ensuring that a future
  // access via a cached mapping sees the up to date value.
  // Note that uncached here refers to any of the uncached policies: device, write combining, etc.
  // Transitioning between different uncached policies does not require a cache operation for
  // correctness, but it is also harmless and not a case we attempt to optimize for.
  cow_pages_locked()->FinishCachePolicyTransitionLocked();

  return ZX_OK;
}

void VmObjectPaged::RangeChangeUpdateLocked(VmCowRange range, RangeChangeOp op) {
  canary_.Assert();
  DEBUG_ASSERT(range.is_page_aligned());

  uint64_t aligned_offset = range.offset;
  uint64_t aligned_len = range.len;
  if (GetIntersect(cow_range_.offset, cow_range_.len, aligned_offset, aligned_len, &aligned_offset,
                   &aligned_len)) {
    // Found the intersection in cow space, convert back to object space.
    aligned_offset -= cow_range_.offset;
    self_locked()->RangeChangeUpdateMappingsLocked(aligned_offset, aligned_len, op);
  }

  // Propagate the change to reference children as well. This is done regardless of intersection as
  // we may have become the holder of the reference list even if they were not originally references
  // made against us, and so their cow views might be different.
  for (auto& ref : reference_list_) {
    AssertHeld(ref.lock_ref());
    // Use the same offset and len. References span the entirety of the parent VMO and hence share
    // all offsets.
    ref.RangeChangeUpdateLocked(range, op);
  }
}

void VmObjectPaged::ForwardRangeChangeUpdateLocked(uint64_t offset, uint64_t len,
                                                   RangeChangeOp op) {
  canary_.Assert();
  DEBUG_ASSERT(IsPageRounded(offset) && IsPageRounded(len));

  // Call RangeChangeUpdateLocked on the owner of the CowPages.
  AssertHeld(cow_pages_locked()->get_paged_backlink_locked()->lock_ref());
  if (auto cow_range = GetCowRange(offset, len)) {
    VmObjectPaged* const paged_owner = cow_pages_locked()->get_paged_backlink_locked();
    cow_pages_locked()->RangeChangeUpdateMappingsLocked(*paged_owner, *cow_range, op);
  }
}

zx_status_t VmObjectPaged::LockRange(uint64_t offset, uint64_t len,
                                     zx_vmo_lock_state_t* lock_state_out) {
  if (!is_discardable()) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  Guard<CriticalMutex> guard{lock()};
  return cow_pages_locked()->LockRangeLocked(*cow_range, lock_state_out);
}

zx_status_t VmObjectPaged::TryLockRange(uint64_t offset, uint64_t len) {
  if (!is_discardable()) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  Guard<CriticalMutex> guard{lock()};
  return cow_pages_locked()->TryLockRangeLocked(*cow_range);
}

zx_status_t VmObjectPaged::UnlockRange(uint64_t offset, uint64_t len) {
  if (!is_discardable()) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  auto cow_range = GetCowRange(offset, len);
  if (!cow_range) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  Guard<CriticalMutex> guard{lock()};
  return cow_pages_locked()->UnlockRangeLocked(*cow_range);
}

zx_status_t VmObjectPaged::GetPage(uint64_t offset, uint pf_flags, MultiPageRequest* page_request,
                                   vm_page_t** page, paddr_t* pa) {
  __UNINITIALIZED VmCowPages::DeferredOps deferred(cow_pages_.get());
  Guard<CriticalMutex> guard{lock()};
  const bool write = pf_flags & VMM_PF_FLAG_WRITE;
  zx::result<VmCowPages::LookupCursor> cursor = GetLookupCursorLocked(offset, kPageSize);
  if (cursor.is_error()) {
    return cursor.error_value();
  }
  AssertHeld(cursor->lock_ref());
  // Hardware faults are considered to update access times separately, all other lookup reasons
  // should do the default update of access time.
  if (pf_flags & VMM_PF_FLAG_HW_FAULT) {
    cursor->DisableMarkAccessed();
  }
  if (!(pf_flags & VMM_PF_FLAG_FAULT_MASK)) {
    vm_page_t* p = cursor->MaybePage(write);
    if (!p) {
      return ZX_ERR_NOT_FOUND;
    }
    if (page) {
      *page = p;
    }
    if (pa) {
      *pa = p->paddr();
    }
    return ZX_OK;
  }
  auto result = cursor->RequirePage(write, /*max_request_pages=*/1, deferred, page_request);
  if (result.is_error()) {
    return result.error_value();
  }
  if (page) {
    *page = result->page;
  }
  if (pa) {
    *pa = result->page->paddr();
  }
  return ZX_OK;
}
