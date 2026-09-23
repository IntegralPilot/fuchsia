// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/vm_cow_pages_ffi.h"

#include <zircon/types.h>

#include <kernel/ffi.h>
#include <ktl/memory.h>

#include "vm/vm_cow_pages.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE zx_status_t cpp_vm_cow_pages_replace_page_with_loaned(VmCowPages* cow,
                                                                        vm_page_t* before_page,
                                                                        uint64_t offset) {
  return cow->ReplacePageWithLoaned(before_page, offset);
}

FFI_ALWAYS_INLINE void* cpp_vm_cow_pages_get_ref_counted(const VmCowPages* cow) {
  return const_cast<fbl::RefCountedUpgradeable<VmCowPages>*>(
      static_cast<const fbl::RefCountedUpgradeable<VmCowPages>*>(cow));
}

FFI_ALWAYS_INLINE void cpp_vm_cow_pages_free(VmCowPages* cow) { delete cow; }

FFI_ALWAYS_INLINE void cpp_vm_cow_pages_initialize_page_cache(uint32_t level) {
  VmCowPages::InitializePageCache(level);
}

FFI_ALWAYS_INLINE PmmOptDelayReuse
cpp_vm_cow_pages_should_delay_reuse_on_free(const VmCowPages* cow) {
  return cow->should_delay_reuse_on_free();
}

FFI_ALWAYS_INLINE VmCowPages* cpp_vm_cow_pages_debug_get_parent(VmCowPages* cow) {
  auto parent = cow->DebugGetParent();
  return fbl::ExportToRawPtr(&parent);
}

FFI_ALWAYS_INLINE bool cpp_vm_cow_pages_dedup_zero_page(VmCowPages* cow, vm_page_t* page,
                                                        uint64_t offset) {
  return cow->DedupZeroPage(page, offset);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_cow_pages_evict_loaned_page(VmCowPages* cow, vm_page_t* page,
                                                                 uint64_t offset) {
  return cow->EvictLoanedPage(page, offset);
}

FFI_ALWAYS_INLINE vm_page_t* cpp_vm_cow_pages_debug_get_page(const VmCowPages* cow,
                                                             uint64_t offset) {
  return cow->DebugGetPage(offset);
}

FFI_ALWAYS_INLINE bool cpp_vm_cow_pages_debug_is_empty(const VmCowPages* cow, uint64_t offset) {
  return cow->DebugIsEmpty(offset);
}

FFI_ALWAYS_INLINE bool cpp_vm_cow_pages_debug_is_high_memory_priority(const VmCowPages* cow) {
  return cow->DebugIsHighMemoryPriority();
}

FFI_ALWAYS_INLINE bool cpp_vm_cow_pages_reclaim_page(
    VmCowPages* cow, vm_page_t* page, uint64_t offset, VmCowPages::EvictionAction eviction_action,
    VmCompressor* compressor, VmCowReclaimSuccess* out_success, VmCowReclaimFailure* out_failure) {
  auto result = cow->ReclaimPage(page, offset, eviction_action, compressor);
  if (result.is_ok()) {
    *out_success = result.value();
    return true;
  }
  *out_failure = result.error_value();
  return false;
}

FFI_ALWAYS_INLINE DiscardableVmoTracker* cpp_vm_cow_pages_debug_get_discardable_tracker(
    const VmCowPages* cow) {
  return cow->DebugGetDiscardableTracker();
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_cow_pages_debug_lookup_readable(
    VmCowPages* cow, VmCowRange range, void* ctx, cpp_vm_cow_pages_lookup_readable_fn callback) {
  return cow->DebugLookupReadable(
      range, [ctx, callback](uint64_t offset, paddr_t pa) { return callback(ctx, offset, pa); });
}

FFI_ALWAYS_INLINE void* cpp_vm_cow_pages_get_lock(const VmCowPages* cow) { return cow->lock(); }

FFI_ALWAYS_INLINE void cpp_vm_cow_pages_deferred_ops_construct(
    ffi::Uninitialized<VmCowPages::DeferredOps>* ops, VmCowPages* cow) {
  ops->Initialize(cow);
}

FFI_ALWAYS_INLINE void cpp_vm_cow_pages_deferred_ops_destroy(VmCowPages::DeferredOps* ops) {
  ktl::destroy_at(ops);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_cow_pages_add_new_pages_locked(
    VmCowPages* cow, uint64_t start_offset, VmPageDoublyLinkedList* pages,
    VmCowPages::CanOverwriteSlot overwrite, bool zero,
    VmCowPages::DeferredOps* deferred) TA_NO_THREAD_SAFETY_ANALYSIS {
  return cow->AddNewPagesLocked(start_offset, pages, overwrite, zero, deferred);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_cow_pages_zero_pages_locked(
    VmCowPages* cow, VmCowRange range, bool dirty_track, VmCowPages::DeferredOps* deferred,
    MultiPageRequest* page_request, uint64_t* out_zeroed_bytes) TA_NO_THREAD_SAFETY_ANALYSIS {
  auto [status, zeroed_bytes] = cow->ZeroPagesLocked(range, dirty_track, *deferred, page_request);
  *out_zeroed_bytes = zeroed_bytes;
  return status;
}

FFI_ALWAYS_INLINE uint32_t cpp_vm_cow_pages_debug_get_populated_slots_count(const VmCowPages* cow) {
  return cow->DebugGetPopulatedSlotsCount();
}

FFI_ALWAYS_INLINE bool cpp_vm_cow_pages_debug_is_parent_content(const VmCowPages* cow,
                                                                uint64_t offset) {
  return cow->DebugIsParentContent(offset);
}

FFI_ALWAYS_INLINE bool cpp_vm_cow_pages_can_borrow(const VmCowPages* cow) {
  return cow->can_borrow();
}

FFI_ALWAYS_INLINE bool cpp_vm_cow_pages_can_evict(const VmCowPages* cow) {
  return cow->can_evict();
}

FFI_ALWAYS_INLINE bool cpp_vm_cow_pages_is_discardable(const VmCowPages* cow) {
  return cow->is_discardable();
}

FFI_ALWAYS_INLINE VmCowPages* cpp_vm_cow_pages_upgrade_from_raw(VmCowPages* cow) {
  if (!cow->AddRefMaybeInDestructor()) {
    return nullptr;
  }
  return cow;
}

}  // extern "C"
