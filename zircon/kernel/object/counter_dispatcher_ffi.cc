// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/ffi.h>
#include <object/counter_dispatcher.h>
#include <object/process_dispatcher.h>

extern "C" {

zx_status_t cpp_counter_dispatcher_create(
    ffi::Uninitialized<KernelHandle<CounterDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) CounterDispatcher);
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

}  // extern "C"
