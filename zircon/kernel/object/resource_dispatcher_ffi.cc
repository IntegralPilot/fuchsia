// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <kernel/ffi.h>
#include <object/handle.h>
#include <object/resource_dispatcher.h>

extern "C" {

zx_status_t cpp_resource_dispatcher_create(
    zx_rsrc_kind_t kind, uint64_t base, size_t size, uint32_t flags, const char* name,
    size_t name_size, void* storage, void* region,
    ffi::Uninitialized<KernelHandle<ResourceDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  KernelHandle handle(fbl::AdoptRef(
      new (&ac) ResourceDispatcher(kind, base, size, flags, name, name_size, storage, region)));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  handle_out->Initialize(ktl::move(handle));
  return ZX_OK;
}

}  // extern "C"
