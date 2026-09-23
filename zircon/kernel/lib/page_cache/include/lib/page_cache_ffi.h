// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_PAGE_CACHE_INCLUDE_LIB_PAGE_CACHE_FFI_H_
#define ZIRCON_KERNEL_LIB_PAGE_CACHE_INCLUDE_LIB_PAGE_CACHE_FFI_H_

#include <zircon/types.h>

#include <cstddef>

#include <vm/page.h>

namespace page_cache {
class PageCache;
}  // namespace page_cache

extern "C" {

zx_status_t cpp_page_cache_create(size_t reserve_pages, page_cache::PageCache** out_cache);
void cpp_page_cache_delete(page_cache::PageCache* cache);
size_t cpp_page_cache_reserve_pages(const page_cache::PageCache* cache);
zx_status_t cpp_page_cache_alloc(page_cache::PageCache* cache, size_t count,
                                 VmPageDoublyLinkedList* out_pages, size_t* out_available_pages);
void cpp_page_cache_free(page_cache::PageCache* cache, VmPageDoublyLinkedList* pages);

}  // extern "C"

#endif  // ZIRCON_KERNEL_LIB_PAGE_CACHE_INCLUDE_LIB_PAGE_CACHE_FFI_H_
