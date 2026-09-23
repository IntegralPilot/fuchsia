// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_VM_ADDRESS_REGION_DISPATCHER_FFI_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_VM_ADDRESS_REGION_DISPATCHER_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/handle.h>
#include <object/vm_address_region_dispatcher.h>

__BEGIN_CDECLS

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t
cpp_vmar_dispatcher_set_memory_priority(VmAddressRegionDispatcher* vmar, uint32_t priority);

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_vmar_dispatcher_allocate(
    VmAddressRegionDispatcher* vmar, size_t offset, size_t size, uint32_t flags,
    ffi::Uninitialized<KernelHandle<VmAddressRegionDispatcher>>* handle_out,
    ffi::Uninitialized<zx_rights_t>* rights_out);

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_vmar_dispatcher_map(VmAddressRegionDispatcher* vmar,
                                                      size_t vmar_offset, VmObject* vmo,
                                                      uint64_t vmo_offset, size_t len,
                                                      uint32_t flags, zx_vaddr_t* out_base);

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_info_vmar_t
cpp_vmar_dispatcher_get_vmar_info(const VmAddressRegionDispatcher* vmar);

__END_CDECLS

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_VM_ADDRESS_REGION_DISPATCHER_FFI_H_
