// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <ktl/utility.h>
#include <object/handle.h>
#include <object/stream_dispatcher.h>
#include <object/vm_object_dispatcher.h>
#include <vm/stream_size_manager.h>
#include <vm/vm_object_paged.h>

extern "C" {

zx_status_t cpp_stream_dispatcher_create(
    uint32_t options, const VmObjectDispatcher* vmo_dispatcher, zx_off_t seek,
    ffi::Uninitialized<KernelHandle<StreamDispatcher>>* handle_out) {
  auto result = const_cast<VmObjectDispatcher*>(vmo_dispatcher)->stream_size_manager();
  if (result.is_error()) {
    return result.status_value();
  }

  fbl::AllocChecker ac;
  auto dispatcher = fbl::AdoptRef(new (&ac) StreamDispatcher(
      options, DownCastVmObject<VmObjectPaged>(vmo_dispatcher->vmo()), ktl::move(*result), seek));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(dispatcher));
  return ZX_OK;
}

}  // extern "C"
