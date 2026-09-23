// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include "vm/pmm.h"

class VmCompression;

__BEGIN_CDECLS

vm_page_t* cpp_paddr_to_vm_page(zx_paddr_t paddr);
PageQueues* cpp_pmm_page_queues();
VmCompression* cpp_pmm_get_page_compression(void);
zx_status_t cpp_pmm_alloc_page(uint32_t flags, vm_page_t** out_page, zx_paddr_t* out_paddr);
zx_status_t cpp_pmm_alloc_pages(size_t count, uint32_t flags, VmPageDoublyLinkedList* list);
zx_status_t cpp_pmm_alloc_contiguous(size_t count, uint32_t flags, uint8_t align_log2,
                                     zx_paddr_t* out_pa, VmPageDoublyLinkedList* list);
void cpp_pmm_free_page(vm_page_t* page);
// Frees every page on `list` back to the PMM, emptying the list.
void cpp_pmm_free_list(VmPageDoublyLinkedList* list);
size_t cpp_pmm_num_arenas();
zx_status_t cpp_pmm_get_arena_info(size_t count, uint64_t i, pmm_arena_info_t* buffer,
                                   size_t buffer_size);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_FFI_H_
