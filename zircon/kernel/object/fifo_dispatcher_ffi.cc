// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/counters.h>

#include <fbl/alloc_checker.h>
#include <kernel/ffi.h>
#include <kernel/lockdep.h>
#include <ktl/utility.h>
#include <object/fifo_dispatcher.h>

extern "C" {

zx_status_t cpp_fifo_dispatcher_create(
    void* holder, uint32_t count, uint32_t elem_size, void* data,
    ffi::Uninitialized<KernelHandle<FifoDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) FifoDispatcher(holder, count, elem_size, data));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

}  // extern "C"
