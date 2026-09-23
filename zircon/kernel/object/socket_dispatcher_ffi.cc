// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/user_copy/user_ptr.h>

#include <fbl/alloc_checker.h>
#include <kernel/ffi.h>
#include <object/socket_dispatcher.h>

extern "C" {

zx_status_t cpp_socket_dispatcher_create(
    void* holder, uint32_t flags, ffi::Uninitialized<KernelHandle<SocketDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) SocketDispatcher(holder, flags));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

}  // extern "C"
