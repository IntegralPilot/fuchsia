// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <dev/iommu/bti.h>
#include <dev/iommu/iommu.h>
#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <ktl/utility.h>
#include <object/bus_transaction_initiator_dispatcher.h>
#include <object/handle.h>

extern "C" {

zx_status_t cpp_bus_transaction_initiator_dispatcher_create(
    const iommu::Iommu* iommu, uint64_t bti_id,
    ffi::Uninitialized<KernelHandle<BusTransactionInitiatorDispatcher>>* handle_out) {
  zx::result<fbl::RefPtr<iommu::Bti>> maybe_bti =
      const_cast<iommu::Iommu*>(iommu)->CreateBti(bti_id);
  if (!maybe_bti.is_ok()) {
    return maybe_bti.error_value();
  }

  fbl::AllocChecker ac;
  KernelHandle new_handle(
      fbl::AdoptRef(new (&ac) BusTransactionInitiatorDispatcher(ktl::move(maybe_bti.value()))));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(new_handle));
  return ZX_OK;
}

}  // extern "C"
