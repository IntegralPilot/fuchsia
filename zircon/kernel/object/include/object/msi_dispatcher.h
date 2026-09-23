// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_MSI_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_MSI_DISPATCHER_H_

#include <lib/object-constants.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/msi_allocation.h>
#include <object/opaque_storage.h>

class MsiDispatcher;

extern "C" {

zx_status_t cpp_msi_dispatcher_create(MsiAllocation* msi_alloc_raw,
                                      ffi::Uninitialized<KernelHandle<MsiDispatcher>>* handle_out);
void rust_msi_dispatcher_state_init(void* storage, void* dispatcher, MsiAllocation* msi_alloc);
void rust_msi_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_msi_dispatcher_state_get_lock(const void* state);
zx_info_msi_t rust_msi_dispatcher_get_info(const MsiDispatcher* disp);

}  // extern "C"

class MsiDispatcher final : public Dispatcher {
 public:
  ~MsiDispatcher() final;

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_MSI; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return false; }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final {
    return UserSignalSelfSolo(this, clear_mask, set_mask, 0);
  }
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  using Dispatcher::UpdateState;
  using Dispatcher::UpdateStateLocked;

  zx_info_msi_t GetInfo() const;

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  friend zx_status_t cpp_msi_dispatcher_create(
      MsiAllocation* msi_alloc_raw, ffi::Uninitialized<KernelHandle<MsiDispatcher>>* handle_out);
  explicit MsiDispatcher(fbl::RefPtr<MsiAllocation> msi_alloc);

  OpaqueStorage<kMsiDispatcherStateSize, kMsiDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_MSI_DISPATCHER_H_
