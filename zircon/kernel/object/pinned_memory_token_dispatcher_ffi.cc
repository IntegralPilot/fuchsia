// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <ktl/utility.h>
#include <object/bus_transaction_initiator_dispatcher.h>
#include <object/handle.h>
#include <object/pinned_memory_token_dispatcher.h>

extern "C" {

zx_status_t cpp_pinned_memory_token_dispatcher_create(
    BusTransactionInitiatorDispatcher* bti,
    ffi::Uninitialized<KernelHandle<PinnedMemoryTokenDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  KernelHandle new_handle(
      fbl::AdoptRef(new (&ac) PinnedMemoryTokenDispatcher(fbl::ImportFromRawPtr(bti))));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(new_handle));
  return ZX_OK;
}

}  // extern "C"
