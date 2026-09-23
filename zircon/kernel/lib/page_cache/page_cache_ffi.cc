// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/page_cache_ffi.h"

#include <lib/page_cache.h>

#include <fbl/alloc_checker.h>
#include <kernel/ffi.h>
#include <ktl/utility.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_page_cache_create(size_t reserve_pages,
                                                    page_cache::PageCache** out_cache) {
  zx::result<page_cache::PageCache> result = page_cache::PageCache::Create(reserve_pages);
  if (result.is_error()) {
    return result.status_value();
  }
  fbl::AllocChecker ac;
  auto* cache = new (&ac) page_cache::PageCache(ktl::move(result.value()));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  *out_cache = cache;
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_page_cache_delete(page_cache::PageCache* cache) { delete cache; }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE size_t cpp_page_cache_reserve_pages(const page_cache::PageCache* cache) {
  return cache->reserve_pages();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_page_cache_alloc(page_cache::PageCache* cache, size_t count,
                                                   VmPageDoublyLinkedList* out_pages,
                                                   size_t* out_available_pages) {
  zx::result<page_cache::PageCache::AllocateResult> result = cache->Allocate(count);
  if (result.is_error()) {
    return result.status_value();
  }
  out_pages->splice(out_pages->end(), result->page_list);
  if (out_available_pages) {
    *out_available_pages = result->available_pages;
  }
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_page_cache_free(page_cache::PageCache* cache,
                                           VmPageDoublyLinkedList* pages) {
  cache->Free(page_cache::PageCache::PageList{ktl::move(*pages)});
}

}  // extern "C"
