// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_COW_PAGES_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_COW_PAGES_FFI_H_

#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/ffi.h>

#include "vm/vm_cow_pages.h"

__BEGIN_CDECLS

typedef zx_status_t (*cpp_vm_cow_pages_lookup_readable_fn)(void* ctx, uint64_t offset,
                                                           uint64_t paddr);

zx_status_t cpp_vm_cow_pages_replace_page_with_loaned(VmCowPages* cow, vm_page_t* before_page,
                                                      uint64_t offset);
void* cpp_vm_cow_pages_get_ref_counted(const VmCowPages* cow);
void cpp_vm_cow_pages_free(VmCowPages* cow);
void cpp_vm_cow_pages_initialize_page_cache(uint32_t level);
PmmOptDelayReuse cpp_vm_cow_pages_should_delay_reuse_on_free(const VmCowPages* cow);
VmCowPages* cpp_vm_cow_pages_debug_get_parent(VmCowPages* cow);
bool cpp_vm_cow_pages_dedup_zero_page(VmCowPages* cow, vm_page_t* page, uint64_t offset);
zx_status_t cpp_vm_cow_pages_evict_loaned_page(VmCowPages* cow, vm_page_t* page, uint64_t offset);
vm_page_t* cpp_vm_cow_pages_debug_get_page(const VmCowPages* cow, uint64_t offset);
bool cpp_vm_cow_pages_debug_is_empty(const VmCowPages* cow, uint64_t offset);
bool cpp_vm_cow_pages_debug_is_high_memory_priority(const VmCowPages* cow);
DiscardableVmoTracker* cpp_vm_cow_pages_debug_get_discardable_tracker(const VmCowPages* cow);
bool cpp_vm_cow_pages_reclaim_page(VmCowPages* cow, vm_page_t* page, uint64_t offset,
                                   VmCowPages::EvictionAction eviction_action,
                                   VmCompressor* compressor, VmCowReclaimSuccess* out_success,
                                   VmCowReclaimFailure* out_failure);
zx_status_t cpp_vm_cow_pages_debug_lookup_readable(VmCowPages* cow, VmCowRange range, void* ctx,
                                                   cpp_vm_cow_pages_lookup_readable_fn callback);

void* cpp_vm_cow_pages_get_lock(const VmCowPages* cow);
void cpp_vm_cow_pages_deferred_ops_construct(ffi::Uninitialized<VmCowPages::DeferredOps>* ops,
                                             VmCowPages* cow);
void cpp_vm_cow_pages_deferred_ops_destroy(VmCowPages::DeferredOps* ops);
zx_status_t cpp_vm_cow_pages_add_new_pages_locked(VmCowPages* cow, uint64_t start_offset,
                                                  VmPageDoublyLinkedList* pages,
                                                  VmCowPages::CanOverwriteSlot overwrite, bool zero,
                                                  VmCowPages::DeferredOps* deferred);
zx_status_t cpp_vm_cow_pages_zero_pages_locked(VmCowPages* cow, VmCowRange range, bool dirty_track,
                                               VmCowPages::DeferredOps* deferred,
                                               MultiPageRequest* page_request,
                                               uint64_t* out_zeroed_bytes);
uint32_t cpp_vm_cow_pages_debug_get_populated_slots_count(const VmCowPages* cow);
bool cpp_vm_cow_pages_debug_is_parent_content(const VmCowPages* cow, uint64_t offset);
bool cpp_vm_cow_pages_can_borrow(const VmCowPages* cow);
bool cpp_vm_cow_pages_can_evict(const VmCowPages* cow);
bool cpp_vm_cow_pages_is_discardable(const VmCowPages* cow);
VmCowPages* cpp_vm_cow_pages_upgrade_from_raw(VmCowPages* cow);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_COW_PAGES_FFI_H_
