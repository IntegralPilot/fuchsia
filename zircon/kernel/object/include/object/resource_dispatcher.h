// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_RESOURCE_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_RESOURCE_DISPATCHER_H_

#include <lib/object-constants.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/rights.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/resource.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class ResourceDispatcher;

extern "C" {
zx_status_t cpp_resource_dispatcher_create(
    zx_rsrc_kind_t kind, uint64_t base, size_t size, uint32_t flags, const char* name,
    size_t name_size, void* storage, void* region,
    ffi::Uninitialized<KernelHandle<ResourceDispatcher>>* handle_out);
void rust_resource_dispatcher_state_init(void* state, void* disp, zx_rsrc_kind_t kind,
                                         uint64_t base, size_t size, uint32_t flags,
                                         const char* name, size_t name_size, void* storage,
                                         void* region);
void rust_resource_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_resource_dispatcher_state_get_lock(const void* state);

zx_status_t rust_resource_dispatcher_create_ranged_root(
    ffi::Uninitialized<KernelHandle<ResourceDispatcher>>* handle_out, zx_rights_t* rights_out,
    zx_rsrc_kind_t kind, const char* name);
zx_status_t rust_resource_dispatcher_initialize_allocator(zx_rsrc_kind_t kind, uint64_t base,
                                                          size_t size);
zx_status_t rust_resource_dispatcher_get_name(const ResourceDispatcher* disp,
                                              char out_name[ZX_MAX_NAME_LEN]);
void rust_resource_dispatcher_get_info(const ResourceDispatcher* disp,
                                       zx_info_resource_t* info_out);
void rust_resource_dispatcher_dump_resources();
void rust_resource_dispatcher_dump_allocators();
}  // extern "C"

class ResourceDispatcher final : public Dispatcher {
 public:
  // Creates ResourceDispatcher object representing access rights to all
  // regions of address space for a ranged resource.
  static zx_status_t CreateRangedRoot(KernelHandle<ResourceDispatcher>* handle, zx_rights_t* rights,
                                      zx_rsrc_kind_t kind, const char name[ZX_MAX_NAME_LEN]);

  // Initializes the static members used for bookkeeping and storage.
  static zx_status_t InitializeAllocator(zx_rsrc_kind_t kind, uint64_t base, size_t size);

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_RESOURCE; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return false; }

  // Returns a null-terminated name.
  [[nodiscard]] zx_status_t get_name(char (&out_name)[ZX_MAX_NAME_LEN]) const final {
    return rust_resource_dispatcher_get_name(this, out_name);
  }

  zx_info_resource_t GetInfo() const {
    zx_info_resource_t info;
    rust_resource_dispatcher_get_info(this, &info);
    return info;
  }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  ~ResourceDispatcher() override;

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  friend zx_status_t cpp_resource_dispatcher_create(
      zx_rsrc_kind_t, uint64_t, size_t, uint32_t, const char*, size_t, void*, void*,
      ffi::Uninitialized<KernelHandle<ResourceDispatcher>>*);
  ResourceDispatcher(zx_rsrc_kind_t kind, uint64_t base, size_t size, uint32_t flags,
                     const char* name, size_t name_size, void* storage, void* region);

  OpaqueStorage<kResourceDispatcherStateSize, kResourceDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_RESOURCE_DISPATCHER_H_
