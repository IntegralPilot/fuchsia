// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/handle.h>
#include <object/vm_address_region_dispatcher.h>
#include <object/vm_address_region_dispatcher_ffi.h>
#include <vm/vm_object.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t
cpp_vmar_dispatcher_set_memory_priority(VmAddressRegionDispatcher* vmar, uint32_t priority) {
  return vmar->SetMemoryPriority(static_cast<VmAddressRegion::MemoryPriority>(priority));
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_vmar_dispatcher_allocate(
    VmAddressRegionDispatcher* vmar, size_t offset, size_t size, uint32_t flags,
    ffi::Uninitialized<KernelHandle<VmAddressRegionDispatcher>>* handle_out,
    ffi::Uninitialized<zx_rights_t>* rights_out) {
  KernelHandle<VmAddressRegionDispatcher> handle;
  zx_rights_t rights;
  zx_status_t status = vmar->Allocate(offset, size, flags, &handle, &rights);
  if (status != ZX_OK) {
    return status;
  }
  handle_out->Initialize(ktl::move(handle));
  rights_out->Initialize(rights);
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_vmar_dispatcher_map(VmAddressRegionDispatcher* vmar,
                                                      size_t vmar_offset, VmObject* vmo,
                                                      uint64_t vmo_offset, size_t len,
                                                      uint32_t flags, zx_vaddr_t* out_base) {
  auto map_result = vmar->Map(vmar_offset, fbl::RefPtr<VmObject>(vmo), vmo_offset, len, flags);
  if (map_result.is_error()) {
    return map_result.status_value();
  }
  *out_base = map_result->base;
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_info_vmar_t
cpp_vmar_dispatcher_get_vmar_info(const VmAddressRegionDispatcher* vmar) {
  return vmar->GetVmarInfo();
}

}  // extern "C"
