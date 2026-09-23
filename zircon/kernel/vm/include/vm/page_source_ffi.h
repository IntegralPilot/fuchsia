// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_SOURCE_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_SOURCE_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <vm/page_source.h>

__BEGIN_CDECLS

void cpp_multi_page_request_construct(MultiPageRequest* req);
void cpp_multi_page_request_destroy(MultiPageRequest* req);
void cpp_multi_page_request_cancel_requests(MultiPageRequest* req);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PAGE_SOURCE_FFI_H_
