// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/resource_dispatcher.h"

#include <lib/console.h>
#include <lib/object-constants.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <object/handle.h>

ResourceDispatcher::ResourceDispatcher(zx_rsrc_kind_t kind, uint64_t base, size_t size,
                                       uint32_t flags, const char* name, size_t name_size,
                                       void* storage, void* region)
    : Dispatcher(0) {
  DISPATCHER_VERIFY_OFFSET(ResourceDispatcher, kResourceDispatcherStateOffset);
  rust_resource_dispatcher_state_init(&opaque_storage_, this, kind, base, size, flags, name,
                                      name_size, storage, region);
}

IMPLEMENT_DISPATCHER_RUST_STATE(ResourceDispatcher, rust_resource_dispatcher_state_get_lock,
                                rust_resource_dispatcher_state_destroy)

zx_status_t ResourceDispatcher::CreateRangedRoot(KernelHandle<ResourceDispatcher>* handle,
                                                 zx_rights_t* rights, zx_rsrc_kind_t kind,
                                                 const char name[ZX_MAX_NAME_LEN]) {
  ffi::Uninitialized<KernelHandle<ResourceDispatcher>> uninit_handle;
  zx_status_t status =
      rust_resource_dispatcher_create_ranged_root(&uninit_handle, rights, kind, name);
  if (status == ZX_OK) {
    *handle = ktl::move(uninit_handle.Get());
  }
  return status;
}

zx_status_t ResourceDispatcher::InitializeAllocator(zx_rsrc_kind_t kind, uint64_t base,
                                                    size_t size) {
  return rust_resource_dispatcher_initialize_allocator(kind, base, size);
}

namespace {

int cmd_resource(int argc, const cmd_args* argv, uint32_t flags) {
  rust_resource_dispatcher_dump_resources();
  rust_resource_dispatcher_dump_allocators();
  return true;
}

}  // namespace

STATIC_COMMAND_START
STATIC_COMMAND("resource", "Inspect physical address space resource allocations", &cmd_resource)
STATIC_COMMAND_END(resource)
