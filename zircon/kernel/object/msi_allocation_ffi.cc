// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/errors.h>

#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <object/msi_allocation.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_msi_allocation_create(uint32_t count, MsiAllocation** alloc_out) {
  fbl::RefPtr<MsiAllocation> alloc;
  zx_status_t status = MsiAllocation::Create(count, &alloc);
  if (status != ZX_OK) {
    return status;
  }
  *alloc_out = fbl::ExportToRawPtr(&alloc);
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_msi_allocation_recycle(MsiAllocation* msi_alloc) { delete msi_alloc; }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_info_msi_t cpp_msi_allocation_get_info(const MsiAllocation* alloc) {
  return alloc->GetInfo();
}

}  // extern "C"
