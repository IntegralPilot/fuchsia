// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <fbl/alloc_checker.h>
#include <kernel/ffi.h>
#include <ktl/utility.h>
#include <object/event_pair_dispatcher.h>

extern "C" {

zx_status_t cpp_event_pair_dispatcher_create(
    void* holder, ffi::Uninitialized<KernelHandle<EventPairDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) EventPairDispatcher(holder));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

}  // extern "C"
