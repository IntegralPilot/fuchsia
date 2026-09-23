// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/page_queues_ffi.h"

#include <zircon/types.h>

#include <fbl/ref_ptr.h>
#include <kernel/event.h>
#include <kernel/ffi.h>
#include <ktl/memory.h>
#include <vm/vm_cow_pages.h>

#include "vm/page_queues.h"
#include "vm/pmm.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE void cpp_page_queues_stop_threads(PageQueues* queues) { queues->StopThreads(); }

FFI_ALWAYS_INLINE void cpp_page_queues_init(ffi::Uninitialized<PageQueues>* queues) {
  queues->Initialize();
}

FFI_ALWAYS_INLINE void cpp_page_queues_set_wired(PageQueues* queues, vm_page_t* page,
                                                 VmCowPages* cow, uint64_t offset) {
  queues->SetWired(page, cow, offset);
}

FFI_ALWAYS_INLINE void cpp_page_queues_set_anonymous(PageQueues* queues, vm_page_t* page,
                                                     VmCowPages* cow, uint64_t offset,
                                                     bool skip_reclaim) {
  queues->SetAnonymous(page, cow, offset, skip_reclaim);
}

FFI_ALWAYS_INLINE void cpp_page_queues_set_reclaim(PageQueues* queues, vm_page_t* page,
                                                   VmCowPages* cow, uint64_t offset) {
  queues->SetReclaim(page, cow, offset);
}

FFI_ALWAYS_INLINE void cpp_page_queues_set_pager_backed_dirty(PageQueues* queues, vm_page_t* page,
                                                              VmCowPages* cow, uint64_t offset) {
  queues->SetPagerBackedDirty(page, cow, offset);
}

FFI_ALWAYS_INLINE void cpp_page_queues_set_anonymous_zero_fork(PageQueues* queues, vm_page_t* page,
                                                               VmCowPages* cow, uint64_t offset) {
  queues->SetAnonymousZeroFork(page, cow, offset);
}

FFI_ALWAYS_INLINE void cpp_page_queues_set_high_priority(PageQueues* queues, vm_page_t* page,
                                                         VmCowPages* cow, uint64_t offset) {
  queues->SetHighPriority(page, cow, offset);
}

FFI_ALWAYS_INLINE void cpp_page_queues_move_to_wired(PageQueues* queues, vm_page_t* page) {
  queues->MoveToWired(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_move_to_anonymous(PageQueues* queues, vm_page_t* page,
                                                         bool skip_reclaim) {
  queues->MoveToAnonymous(page, skip_reclaim);
}

FFI_ALWAYS_INLINE void cpp_page_queues_move_to_reclaim(PageQueues* queues, vm_page_t* page) {
  queues->MoveToReclaim(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_move_to_reclaim_dont_need(PageQueues* queues,
                                                                 vm_page_t* page) {
  queues->MoveToReclaimDontNeed(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_move_to_pager_backed_dirty(PageQueues* queues,
                                                                  vm_page_t* page) {
  queues->MoveToPagerBackedDirty(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_move_to_high_priority(PageQueues* queues, vm_page_t* page) {
  queues->MoveToHighPriority(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_move_anonymous_to_anonymous_zero_fork(PageQueues* queues,
                                                                             vm_page_t* page) {
  queues->MoveAnonymousToAnonymousZeroFork(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_compress_failed(PageQueues* queues, vm_page_t* page) {
  queues->CompressFailed(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_change_object_offset(PageQueues* queues, vm_page_t* page,
                                                            VmCowPages* cow, uint64_t page_offset) {
  queues->ChangeObjectOffset(page, cow, page_offset);
}

FFI_ALWAYS_INLINE void cpp_page_queues_change_object_offset_array(
    PageQueues* queues, vm_page_t** pages, VmCowPages* cow, const uint64_t* offsets, size_t count) {
  queues->ChangeObjectOffsetArray(pages, cow, const_cast<uint64_t*>(offsets), count);
}

FFI_ALWAYS_INLINE void cpp_page_queues_change_object_offset_locked_list(
    PageQueues* queues, vm_page_t* page, VmCowPages* cow,
    uint64_t offset) TA_NO_THREAD_SAFETY_ANALYSIS {
  queues->ChangeObjectOffsetLockedList(page, cow, offset);
}

FFI_ALWAYS_INLINE void cpp_page_queues_remove(PageQueues* queues, vm_page_t* page) {
  queues->Remove(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_remove_array_into_list(PageQueues* queues, vm_page_t** pages,
                                                              size_t count,
                                                              VmPageDoublyLinkedList* out_list) {
  queues->RemoveArrayIntoList(pages, count, out_list);
}

FFI_ALWAYS_INLINE void cpp_page_queues_mark_accessed(PageQueues* queues, vm_page_t* page) {
  queues->MarkAccessed(page);
}

FFI_ALWAYS_INLINE void* cpp_page_queues_get_lock(PageQueues* queues) { return queues->get_lock(); }

FFI_ALWAYS_INLINE const char* cpp_page_queues_string_from_age_reason(int32_t reason) {
  return PageQueues::string_from_age_reason(static_cast<PageQueues::AgeReason>(reason));
}

FFI_ALWAYS_INLINE void cpp_page_queues_rotate_reclaim_queues(PageQueues* queues) {
  queues->RotateReclaimQueues();
}

FFI_ALWAYS_INLINE bool cpp_page_queues_pop_anonymous_zero_fork(
    PageQueues* queues, PageQueuesVmoBacklink* out_backlink) {
  auto backlink = queues->PopAnonymousZeroFork();
  if (!backlink.has_value()) {
    return false;
  }
  out_backlink->cow = fbl::ExportToRawPtr(&backlink->cow);
  out_backlink->page = backlink->page;
  out_backlink->offset = backlink->offset;
  return true;
}

FFI_ALWAYS_INLINE bool cpp_page_queues_peek_isolate(PageQueues* queues, size_t lowest_queue,
                                                    PageQueuesVmoBacklink* out_backlink) {
  auto backlink = queues->PeekIsolate(lowest_queue);
  if (!backlink.has_value()) {
    return false;
  }
  out_backlink->cow = fbl::ExportToRawPtr(&backlink->cow);
  out_backlink->page = backlink->page;
  out_backlink->offset = backlink->offset;
  return true;
}

FFI_ALWAYS_INLINE bool cpp_page_queues_get_cow_for_loaned_page(
    PageQueues* queues, vm_page_t* page, PageQueuesVmoBacklink* out_backlink) {
  auto backlink = queues->GetCowForLoanedPage(page);
  if (!backlink.has_value()) {
    return false;
  }
  out_backlink->cow = fbl::ExportToRawPtr(&backlink->cow);
  out_backlink->page = backlink->page;
  out_backlink->offset = backlink->offset;
  return true;
}

FFI_ALWAYS_INLINE void cpp_page_queues_get_reclaim_queue_counts(
    const PageQueues* queues, PageQueues::ReclaimCounts* out_counts) {
  *out_counts = queues->GetReclaimQueueCounts();
}

FFI_ALWAYS_INLINE void cpp_page_queues_queue_counts(const PageQueues* queues,
                                                    PageQueues::Counts* out_counts) {
  *out_counts = queues->QueueCounts();
}

FFI_ALWAYS_INLINE void cpp_page_queues_get_active_inactive_counts(
    const PageQueues* queues, PageQueues::ActiveInactiveCounts* out_counts) {
  *out_counts = queues->GetActiveInactiveCounts();
}

FFI_ALWAYS_INLINE void cpp_page_queues_dump(PageQueues* queues) { queues->Dump(); }

FFI_ALWAYS_INLINE uint64_t cpp_page_queues_get_lru_pages_compressed(void) {
  return PageQueues::GetLruPagesCompressed();
}

FFI_ALWAYS_INLINE void cpp_page_queues_enable_anonymous_reclaim(PageQueues* queues,
                                                                bool zero_forks) {
  queues->EnableAnonymousReclaim(zero_forks);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_reclaim_is_only_pager_backed(const PageQueues* queues) {
  return queues->ReclaimIsOnlyPagerBacked();
}

FFI_ALWAYS_INLINE bool cpp_page_queues_is_page_reclaimable(const vm_page_t* page) {
  return PageQueues::IsPageReclaimable(page);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_reclaim(const PageQueues* queues,
                                                             const vm_page_t* page,
                                                             size_t* out_queue) {
  return queues->DebugPageIsReclaim(page, out_queue);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_reclaim_isolate(const PageQueues* queues,
                                                                     const vm_page_t* page) {
  return queues->DebugPageIsReclaimIsolate(page);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_pager_backed_dirty(const PageQueues* queues,
                                                                        const vm_page_t* page) {
  return queues->DebugPageIsPagerBackedDirty(page);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_anonymous(const PageQueues* queues,
                                                               const vm_page_t* page) {
  return queues->DebugPageIsAnonymous(page);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_anonymous_zero_fork(const PageQueues* queues,
                                                                         const vm_page_t* page) {
  return queues->DebugPageIsAnonymousZeroFork(page);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_any_anonymous(const PageQueues* queues,
                                                                   const vm_page_t* page) {
  return queues->DebugPageIsAnyAnonymous(page);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_wired(const PageQueues* queues,
                                                           const vm_page_t* page) {
  return queues->DebugPageIsWired(page);
}

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_high_priority(const PageQueues* queues,
                                                                   const vm_page_t* page) {
  return queues->DebugPageIsHighPriority(page);
}

FFI_ALWAYS_INLINE void cpp_page_queues_start_threads(PageQueues* queues,
                                                     zx_duration_mono_t min_mru_rotate_time,
                                                     zx_duration_mono_t max_mru_rotate_time) {
  queues->StartThreads(min_mru_rotate_time, max_mru_rotate_time);
}

FFI_ALWAYS_INLINE void cpp_page_queues_start_debug_compressor(PageQueues* queues) {
  queues->StartDebugCompressor();
}

FFI_ALWAYS_INLINE void cpp_page_queues_set_active_ratio_multiplier(PageQueues* queues,
                                                                   uint32_t multiplier) {
  queues->SetActiveRatioMultiplier(multiplier);
}

FFI_ALWAYS_INLINE void cpp_page_queues_set_lru_action(PageQueues* queues, int32_t action) {
  queues->SetLruAction(static_cast<PageQueues::LruAction>(action));
}

FFI_ALWAYS_INLINE void cpp_page_queues_disable_aging(PageQueues* queues) { queues->DisableAging(); }

FFI_ALWAYS_INLINE void cpp_page_queues_enable_aging(PageQueues* queues) { queues->EnableAging(); }

FFI_ALWAYS_INLINE void cpp_page_queues_set_aging_event(PageQueues* queues, Event* event) {
  queues->SetAgingEvent(event);
}

FFI_ALWAYS_INLINE Thread* cpp_page_queues_debug_get_lru_thread(PageQueues* queues) {
  return queues->DebugGetLruThread();
}

FFI_ALWAYS_INLINE Thread* cpp_page_queues_debug_get_mru_thread(PageQueues* queues) {
  return queues->DebugGetMruThread();
}

FFI_ALWAYS_INLINE void cpp_debug_compressor_init(
    ffi::Uninitialized<VmDebugCompressor>* compressor) {
  compressor->Initialize();
}

FFI_ALWAYS_INLINE void cpp_debug_compressor_destroy(VmDebugCompressor* compressor) {
  ktl::destroy_at(compressor);
}

}  // extern "C"
