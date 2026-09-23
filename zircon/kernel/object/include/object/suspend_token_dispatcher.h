// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SUSPEND_TOKEN_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SUSPEND_TOKEN_DISPATCHER_H_

#include <lib/object-constants.h>
#include <sys/types.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class SuspendTokenDispatcher;
extern "C" {
zx_status_t cpp_suspend_token_dispatcher_create(
    ffi::Uninitialized<KernelHandle<SuspendTokenDispatcher>>* handle_out);
void rust_suspend_token_dispatcher_state_init(void* state, void* disp);
void rust_suspend_token_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_suspend_token_dispatcher_state_get_lock(const void* state);
void rust_suspend_token_dispatcher_on_zero_handles(const SuspendTokenDispatcher* disp);
}

class SuspendTokenDispatcher final : public Dispatcher {
 public:
  ~SuspendTokenDispatcher() final;

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_SUSPEND_TOKEN; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return true; }

  void on_zero_handles() final;

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final;
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  using Dispatcher::UpdateState;
  using Dispatcher::UpdateStateLocked;

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  friend zx_status_t cpp_suspend_token_dispatcher_create(
      ffi::Uninitialized<KernelHandle<SuspendTokenDispatcher>>*);
  SuspendTokenDispatcher();

  OpaqueStorage<kSuspendTokenDispatcherStateSize, kSuspendTokenDispatcherStateAlign>
      opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SUSPEND_TOKEN_DISPATCHER_H_
