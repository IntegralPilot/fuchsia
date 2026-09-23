// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_NODE_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_NODE_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/ffi.h>

#include "vm/pmm_node.h"

class VmCompression;

__BEGIN_CDECLS

void rust_pmm_node_end_handoff(PmmNode* node);
zx_status_t rust_pmm_node_alloc_page(PmmNode* node, uint32_t alloc_flags, vm_page_t** out_page);
zx_status_t rust_pmm_node_alloc_pages(PmmNode* node, size_t count, uint32_t alloc_flags,
                                      VmPageDoublyLinkedList* list);
zx_status_t rust_pmm_node_alloc_range(PmmNode* node, paddr_t address, size_t count,
                                      VmPageDoublyLinkedList* list);
zx_status_t rust_pmm_node_alloc_contiguous(PmmNode* node, size_t count, uint32_t alloc_flags,
                                           uint8_t alignment_log2, paddr_t* pa,
                                           VmPageDoublyLinkedList* list);
void rust_pmm_node_free_page(PmmNode* node, vm_page_t* page, PmmOptDelayReuse delay_reuse);
void rust_pmm_node_free_list(PmmNode* node, VmPageDoublyLinkedList* list,
                             PmmOptDelayReuse delay_reuse);
void rust_pmm_node_with_loaned_page(PmmNode* node, vm_page_t* page,
                                    void (*with_page)(vm_page_t*, void* cookie), void* cookie);
zx_status_t rust_pmm_node_alloc_loaned_page(PmmNode* node,
                                            void (*allocated)(vm_page_t*, void* cookie),
                                            void* cookie, vm_page_t** out_page);
void rust_pmm_node_begin_free_loaned_page(PmmNode* node, vm_page_t* page,
                                          void (*release_page)(vm_page_t*, void* cookie),
                                          void* cookie, FreeLoanedPagesHolder* flph);
void rust_pmm_node_finish_free_loaned_pages(PmmNode* node, FreeLoanedPagesHolder* flph);
void rust_pmm_node_begin_free_loaned_array(PmmNode* node, vm_page_t** pages, size_t count,
                                           void (*release_list)(vm_page_t**, size_t,
                                                                VmPageDoublyLinkedList*,
                                                                void* cookie),
                                           void* cookie, FreeLoanedPagesHolder* flph);
void rust_pmm_node_unwire_page(PmmNode* node, vm_page_t* page);
void rust_pmm_node_begin_loan(PmmNode* node, VmPageDoublyLinkedList* page_list,
                              PmmOptDelayReuse delay_reuse);
void rust_pmm_node_cancel_loan(PmmNode* node, vm_page_t* page);
void rust_pmm_node_end_loan(PmmNode* node, vm_page_t* page);
bool rust_pmm_node_set_free_memory_signal(PmmNode* node, uint64_t free_lower_bound,
                                          uint64_t free_upper_bound,
                                          uint64_t delay_allocations_pages, Event* event);
zx_status_t rust_pmm_node_wait_for_single_page_allocation(PmmNode* node, zx_instant_mono_t deadline,
                                                          zx_duration_t slack_amount,
                                                          slack_mode slack_mode, bool suspendable,
                                                          vm_page_t** out_page);
void rust_pmm_node_stop_returning_should_wait(PmmNode* node);
uint64_t rust_pmm_node_count_free_pages(const PmmNode* node);
uint64_t rust_pmm_node_count_loaned_free_pages(const PmmNode* node);
uint64_t rust_pmm_node_count_loan_cancelled_pages(const PmmNode* node);
uint64_t rust_pmm_node_count_loaned_not_free_pages(const PmmNode* node);
uint64_t rust_pmm_node_count_loaned_pages(const PmmNode* node);
uint64_t rust_pmm_node_count_total_bytes(const PmmNode* node);
void rust_pmm_node_dump_free(const PmmNode* node);
void rust_pmm_node_dump(const PmmNode* node, bool is_panic);
zx_status_t rust_pmm_node_get_arena_info(const PmmNode* node, size_t count, uint64_t i,
                                         pmm_arena_info_t* buffer, size_t buffer_size);
void rust_pmm_node_add_free_pages(PmmNode* node, VmPageDoublyLinkedList* list);
zx_status_t rust_pmm_node_set_page_compression(PmmNode* node, VmCompression* compression);
void rust_pmm_node_fill_free_pages_and_arm(PmmNode* node);
void rust_pmm_node_check_all_free_pages(PmmNode* node);
void rust_pmm_node_poison_all_free_pages(PmmNode* node);
bool rust_pmm_node_enable_free_page_filling(PmmNode* node, size_t fill_size, uint8_t action);
int64_t rust_pmm_node_get_alloc_failed_count();
void rust_pmm_node_seed_random_should_wait(PmmNode* node);
void rust_pmm_node_report_alloc_failure(PmmNode* node, const PmmNode::AllocFailure* failure);
void rust_pmm_node_get_first_alloc_failure(const PmmNode* node, PmmNode::AllocFailure* out_failure);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_NODE_FFI_H_
