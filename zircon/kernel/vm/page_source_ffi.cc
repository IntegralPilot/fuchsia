// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/page_source_ffi.h"

#include <kernel/ffi.h>
#include <ktl/memory.h>

#include "vm/page_source.h"

extern "C" {

FFI_ALWAYS_INLINE void cpp_multi_page_request_construct(MultiPageRequest* req) {
  ktl::construct_at(req);
}

FFI_ALWAYS_INLINE void cpp_multi_page_request_destroy(MultiPageRequest* req) {
  ktl::destroy_at(req);
}

FFI_ALWAYS_INLINE void cpp_multi_page_request_cancel_requests(MultiPageRequest* req) {
  req->CancelRequests();
}

}  // extern "C"
