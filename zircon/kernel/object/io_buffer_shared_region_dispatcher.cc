// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/io_buffer_shared_region_dispatcher.h"

#include <fbl/alloc_checker.h>
#include <vm/vm_address_region.h>
#include <vm/vm_object_paged.h>

extern "C" {
void rust_io_buffer_shared_region_dispatcher_state_init(void* state,
                                                        const IoBufferSharedRegionDispatcher* disp,
                                                        const fbl::RefPtr<VmObjectPaged>& vmo,
                                                        const fbl::RefPtr<VmMapping>& mapping,
                                                        vaddr_t base);
void rust_io_buffer_shared_region_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_io_buffer_shared_region_dispatcher_state_get_lock(const void* state);
}  // extern "C"

IoBufferSharedRegionDispatcher::IoBufferSharedRegionDispatcher(
    const fbl::RefPtr<VmObjectPaged>& vmo, const fbl::RefPtr<VmMapping>& mapping, vaddr_t base)
    : Dispatcher(0u) {
  DISPATCHER_VERIFY_OFFSET(IoBufferSharedRegionDispatcher,
                           kIoBufferSharedRegionDispatcherStateOffset);
  rust_io_buffer_shared_region_dispatcher_state_init(&opaque_storage_, this, vmo, mapping, base);
}

IMPLEMENT_DISPATCHER_RUST_STATE(IoBufferSharedRegionDispatcher,
                                rust_io_buffer_shared_region_dispatcher_state_get_lock,
                                rust_io_buffer_shared_region_dispatcher_state_destroy)

extern "C" zx_status_t cpp_io_buffer_shared_region_dispatcher_create(
    const fbl::RefPtr<VmObjectPaged>& vmo, const fbl::RefPtr<VmMapping>& mapping, vaddr_t base,
    ffi::Uninitialized<KernelHandle<IoBufferSharedRegionDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) IoBufferSharedRegionDispatcher(vmo, mapping, base));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}
