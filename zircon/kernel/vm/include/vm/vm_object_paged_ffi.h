// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PAGED_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PAGED_FFI_H_

#include <lib/user_copy/user_iovec.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include "vm/vm_object_paged.h"

__BEGIN_CDECLS

VmObjectPaged* cpp_vm_object_paged_create(uint32_t pmm_alloc_flags, uint32_t options, uint64_t size,
                                          zx_status_t* out_status);
VmObjectPaged* cpp_vm_object_paged_create_contiguous(uint32_t pmm_alloc_flags, uint64_t size,
                                                     uint8_t alignment_log2,
                                                     zx_status_t* out_status);
VmObject* cpp_vm_object_paged_as_vm_object(VmObjectPaged* vmo);
VmObjectPaged* cpp_vm_object_as_vm_object_paged(VmObject* vmo);
VmCowPages* cpp_vm_object_paged_debug_get_cow_pages(VmObjectPaged* vmo);
vm_page_t* cpp_vm_object_paged_debug_get_page(VmObjectPaged* vmo, uint64_t offset);

zx_status_t cpp_vm_object_paged_read_user_vector(const VmObjectPaged* vmo,
                                                 user_out_ptr<zx_iovec_t> vector, size_t count,
                                                 uint64_t offset, size_t length,
                                                 size_t* out_actual);
zx_status_t cpp_vm_object_paged_write_user_vector(const VmObjectPaged* vmo,
                                                  user_in_ptr<const zx_iovec_t> vector,
                                                  size_t count, uint64_t offset, size_t length,
                                                  size_t* out_actual);
zx_status_t cpp_vm_object_paged_write_user_vector_progress(
    const VmObjectPaged* vmo, user_in_ptr<const zx_iovec_t> vector, size_t count, uint64_t offset,
    size_t length, uint64_t prev_stream_size, size_t* out_actual,
    void (*cb)(void*, uint64_t, size_t), void* cookie);
zx_status_t cpp_vm_object_paged_zero_range(const VmObjectPaged* vmo, uint64_t offset,
                                           uint64_t length);
zx_status_t cpp_vm_object_paged_zero_range_untracked(const VmObjectPaged* vmo, uint64_t offset,
                                                     uint64_t length);
zx_status_t cpp_vm_object_paged_resize(VmObjectPaged* vmo, uint64_t size);
void cpp_vm_object_paged_unmap_and_call(VmObjectPaged* vmo, uint64_t offset, uint64_t len,
                                        void (*call_fn)(void* ctx), void* call_ctx);
void cpp_vm_object_paged_set_user_stream_size(VmObjectPaged* vmo, StreamSizeManager* ssm);
bool cpp_vm_object_paged_user_stream_size(const VmObjectPaged* vmo, uint64_t* out_stream_size);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PAGED_FFI_H_
