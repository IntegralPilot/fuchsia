// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <string.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/iommu.h>

#include <dev/iommu/iommu.h>
#include <dev/iommu/stub/stub.h>
#if ARCH_ARM64
#include <dev/arm_smmu/smmu.h>
#endif
#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <ktl/unique_ptr.h>
#include <ktl/utility.h>
#include <object/handle.h>
#include <object/iommu_dispatcher.h>

extern "C" {

zx_status_t cpp_iommu_dispatcher_create(
    uint32_t type, const uint8_t* desc_ptr, size_t desc_len,
    ffi::Uninitialized<KernelHandle<IommuDispatcher>>* handle_out) {
  ktl::unique_ptr<const uint8_t[]> desc(desc_ptr);
  zx::result<fbl::RefPtr<iommu::Iommu>> result;
  switch (type) {
    case ZX_IOMMU_TYPE_STUB:
      // TODO(b/462772483) Remove this check (or convert it to check for
      // nullptr/0) once we have removed the need to pass a zx_iommu_desc_stub_t
      // at all.
      if (!desc || (desc_len != sizeof(zx_iommu_desc_stub_t))) {
        result = zx::error(ZX_ERR_INVALID_ARGS);
      } else {
        result = StubIommu::Create();
      }
      break;
#if ARCH_ARM64
    case ZX_IOMMU_TYPE_ARM_SMMU:
      result = ArmSmmu::Fetch(ktl::move(desc), desc_len);
      break;
#endif
    default:
      return ZX_ERR_NOT_SUPPORTED;
  }

  if (result.is_error()) {
    return result.error_value();
  }

  fbl::AllocChecker ac;
  KernelHandle new_handle(fbl::AdoptRef(new (&ac) IommuDispatcher(ktl::move(result.value()))));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(new_handle));
  return ZX_OK;
}

}  // extern "C"
