// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/pmm_ffi.h"

#include <zircon/types.h>

#include <kernel/ffi.h>

#include "vm/pmm.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE vm_page_t* cpp_paddr_to_vm_page(zx_paddr_t paddr) {
  return paddr_to_vm_page(paddr);
}

FFI_ALWAYS_INLINE PageQueues* cpp_pmm_page_queues() { return pmm_page_queues(); }

FFI_ALWAYS_INLINE VmCompression* cpp_pmm_get_page_compression(void) {
  return Pmm::Node().GetPageCompression();
}

FFI_ALWAYS_INLINE zx_status_t cpp_pmm_alloc_page(uint32_t flags, vm_page_t** out_page,
                                                 zx_paddr_t* out_paddr) {
  vm_page_t* page = nullptr;
  paddr_t paddr = 0;
  zx_status_t status = pmm_alloc_page(flags, &page, &paddr);
  *out_page = page;
  *out_paddr = paddr;
  return status;
}

FFI_ALWAYS_INLINE zx_status_t cpp_pmm_alloc_pages(size_t count, uint32_t flags,
                                                  VmPageDoublyLinkedList* list) {
  return pmm_alloc_pages(count, flags, list);
}

FFI_ALWAYS_INLINE zx_status_t cpp_pmm_alloc_contiguous(size_t count, uint32_t flags,
                                                       uint8_t align_log2, zx_paddr_t* out_pa,
                                                       VmPageDoublyLinkedList* list) {
  paddr_t pa = 0;
  zx_status_t status = pmm_alloc_contiguous(count, flags, align_log2, &pa, list);
  *out_pa = pa;
  return status;
}

FFI_ALWAYS_INLINE void cpp_pmm_free_page(vm_page_t* page) { pmm_free_page(page); }

FFI_ALWAYS_INLINE void cpp_pmm_free_list(VmPageDoublyLinkedList* list) { pmm_free(list); }

FFI_ALWAYS_INLINE size_t cpp_pmm_num_arenas() { return pmm_num_arenas(); }

FFI_ALWAYS_INLINE zx_status_t cpp_pmm_get_arena_info(size_t count, uint64_t i,
                                                     pmm_arena_info_t* buffer, size_t buffer_size) {
  return pmm_get_arena_info(count, i, buffer, buffer_size);
}

}  // extern "C"
