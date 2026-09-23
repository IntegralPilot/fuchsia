// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/errors.h>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <ktl/utility.h>
#include <object/handle.h>
#include <object/msi_allocation.h>
#include <object/msi_dispatcher.h>

extern "C" {

zx_status_t cpp_msi_dispatcher_create(MsiAllocation* msi_alloc_raw,
                                      ffi::Uninitialized<KernelHandle<MsiDispatcher>>* handle_out) {
  if (!msi_alloc_raw) {
    return ZX_ERR_INVALID_ARGS;
  }

  fbl::RefPtr<MsiAllocation> msi_alloc = fbl::ImportFromRawPtr(msi_alloc_raw);
  fbl::AllocChecker ac;
  KernelHandle new_handle(fbl::AdoptRef(new (&ac) MsiDispatcher(ktl::move(msi_alloc))));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(new_handle));
  return ZX_OK;
}

}  // extern "C"
