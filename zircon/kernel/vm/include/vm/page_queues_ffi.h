// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_QUEUES_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_QUEUES_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <kernel/thread.h>

#include "vm/debug_compressor.h"
#include "vm/page_queues.h"

// Version of PageQueues::VmoBacklink that uses a raw pointer instead of a refptr for FFI.
struct PageQueuesVmoBacklink {
  VmCowPages* cow;
  vm_page_t* page;
  uint64_t offset;
};

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
__BEGIN_CDECLS

FFI_ALWAYS_INLINE void cpp_page_queues_stop_threads(PageQueues* queues);
FFI_ALWAYS_INLINE void cpp_page_queues_init(ffi::Uninitialized<PageQueues>* queues);

FFI_ALWAYS_INLINE void cpp_page_queues_set_wired(PageQueues* queues, vm_page_t* page,
                                                 VmCowPages* cow, uint64_t offset);
FFI_ALWAYS_INLINE void cpp_page_queues_set_anonymous(PageQueues* queues, vm_page_t* page,
                                                     VmCowPages* cow, uint64_t offset,
                                                     bool skip_reclaim);
FFI_ALWAYS_INLINE void cpp_page_queues_set_reclaim(PageQueues* queues, vm_page_t* page,
                                                   VmCowPages* cow, uint64_t offset);
FFI_ALWAYS_INLINE void cpp_page_queues_set_pager_backed_dirty(PageQueues* queues, vm_page_t* page,
                                                              VmCowPages* cow, uint64_t offset);
FFI_ALWAYS_INLINE void cpp_page_queues_set_anonymous_zero_fork(PageQueues* queues, vm_page_t* page,
                                                               VmCowPages* cow, uint64_t offset);
FFI_ALWAYS_INLINE void cpp_page_queues_set_high_priority(PageQueues* queues, vm_page_t* page,
                                                         VmCowPages* cow, uint64_t offset);

FFI_ALWAYS_INLINE void cpp_page_queues_move_to_wired(PageQueues* queues, vm_page_t* page);
FFI_ALWAYS_INLINE void cpp_page_queues_move_to_anonymous(PageQueues* queues, vm_page_t* page,
                                                         bool skip_reclaim);
FFI_ALWAYS_INLINE void cpp_page_queues_move_to_reclaim(PageQueues* queues, vm_page_t* page);
FFI_ALWAYS_INLINE void cpp_page_queues_move_to_reclaim_dont_need(PageQueues* queues,
                                                                 vm_page_t* page);
FFI_ALWAYS_INLINE void cpp_page_queues_move_to_pager_backed_dirty(PageQueues* queues,
                                                                  vm_page_t* page);
FFI_ALWAYS_INLINE void cpp_page_queues_move_to_high_priority(PageQueues* queues, vm_page_t* page);
FFI_ALWAYS_INLINE void cpp_page_queues_move_anonymous_to_anonymous_zero_fork(PageQueues* queues,
                                                                             vm_page_t* page);

FFI_ALWAYS_INLINE void cpp_page_queues_compress_failed(PageQueues* queues, vm_page_t* page);

FFI_ALWAYS_INLINE void cpp_page_queues_change_object_offset(PageQueues* queues, vm_page_t* page,
                                                            VmCowPages* cow, uint64_t page_offset);
FFI_ALWAYS_INLINE void cpp_page_queues_change_object_offset_array(
    PageQueues* queues, vm_page_t** pages, VmCowPages* cow, const uint64_t* offsets, size_t count);
FFI_ALWAYS_INLINE void cpp_page_queues_change_object_offset_locked_list(PageQueues* queues,
                                                                        vm_page_t* page,
                                                                        VmCowPages* cow,
                                                                        uint64_t offset);

FFI_ALWAYS_INLINE void cpp_page_queues_remove(PageQueues* queues, vm_page_t* page);
FFI_ALWAYS_INLINE void cpp_page_queues_remove_array_into_list(PageQueues* queues, vm_page_t** pages,
                                                              size_t count,
                                                              VmPageDoublyLinkedList* out_list);

FFI_ALWAYS_INLINE void cpp_page_queues_mark_accessed(PageQueues* queues, vm_page_t* page);

FFI_ALWAYS_INLINE void* cpp_page_queues_get_lock(PageQueues* queues);

FFI_ALWAYS_INLINE const char* cpp_page_queues_string_from_age_reason(int32_t reason);

FFI_ALWAYS_INLINE void cpp_page_queues_rotate_reclaim_queues(PageQueues* queues);

FFI_ALWAYS_INLINE bool cpp_page_queues_pop_anonymous_zero_fork(PageQueues* queues,
                                                               PageQueuesVmoBacklink* out_backlink);
FFI_ALWAYS_INLINE bool cpp_page_queues_peek_isolate(PageQueues* queues, size_t lowest_queue,
                                                    PageQueuesVmoBacklink* out_backlink);
FFI_ALWAYS_INLINE bool cpp_page_queues_get_cow_for_loaned_page(PageQueues* queues, vm_page_t* page,
                                                               PageQueuesVmoBacklink* out_backlink);

FFI_ALWAYS_INLINE void cpp_page_queues_get_reclaim_queue_counts(
    const PageQueues* queues, PageQueues::ReclaimCounts* out_counts);
FFI_ALWAYS_INLINE void cpp_page_queues_queue_counts(const PageQueues* queues,
                                                    PageQueues::Counts* out_counts);
FFI_ALWAYS_INLINE void cpp_page_queues_get_active_inactive_counts(
    const PageQueues* queues, PageQueues::ActiveInactiveCounts* out_counts);

FFI_ALWAYS_INLINE void cpp_page_queues_dump(PageQueues* queues);

FFI_ALWAYS_INLINE uint64_t cpp_page_queues_get_lru_pages_compressed(void);

FFI_ALWAYS_INLINE void cpp_page_queues_enable_anonymous_reclaim(PageQueues* queues,
                                                                bool zero_forks);
FFI_ALWAYS_INLINE bool cpp_page_queues_reclaim_is_only_pager_backed(const PageQueues* queues);
FFI_ALWAYS_INLINE bool cpp_page_queues_is_page_reclaimable(const vm_page_t* page);

FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_reclaim(const PageQueues* queues,
                                                             const vm_page_t* page,
                                                             size_t* out_queue);
FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_reclaim_isolate(const PageQueues* queues,
                                                                     const vm_page_t* page);
FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_pager_backed_dirty(const PageQueues* queues,
                                                                        const vm_page_t* page);
FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_anonymous(const PageQueues* queues,
                                                               const vm_page_t* page);
FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_anonymous_zero_fork(const PageQueues* queues,
                                                                         const vm_page_t* page);
FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_any_anonymous(const PageQueues* queues,
                                                                   const vm_page_t* page);
FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_wired(const PageQueues* queues,
                                                           const vm_page_t* page);
FFI_ALWAYS_INLINE bool cpp_page_queues_debug_page_is_high_priority(const PageQueues* queues,
                                                                   const vm_page_t* page);

FFI_ALWAYS_INLINE void cpp_page_queues_start_threads(PageQueues* queues,
                                                     zx_duration_mono_t min_mru_rotate_time,
                                                     zx_duration_mono_t max_mru_rotate_time);
FFI_ALWAYS_INLINE void cpp_page_queues_start_debug_compressor(PageQueues* queues);
FFI_ALWAYS_INLINE void cpp_page_queues_set_active_ratio_multiplier(PageQueues* queues,
                                                                   uint32_t multiplier);
FFI_ALWAYS_INLINE void cpp_page_queues_set_lru_action(PageQueues* queues, int32_t action);
FFI_ALWAYS_INLINE void cpp_page_queues_disable_aging(PageQueues* queues);
FFI_ALWAYS_INLINE void cpp_page_queues_enable_aging(PageQueues* queues);
FFI_ALWAYS_INLINE void cpp_page_queues_set_aging_event(PageQueues* queues, Event* event);

FFI_ALWAYS_INLINE Thread* cpp_page_queues_debug_get_lru_thread(PageQueues* queues);
FFI_ALWAYS_INLINE Thread* cpp_page_queues_debug_get_mru_thread(PageQueues* queues);

FFI_ALWAYS_INLINE void cpp_debug_compressor_init(ffi::Uninitialized<VmDebugCompressor>* compressor);
FFI_ALWAYS_INLINE void cpp_debug_compressor_destroy(VmDebugCompressor* compressor);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_QUEUES_FFI_H_
