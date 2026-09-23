// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_COUNTER_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_COUNTER_DISPATCHER_H_

#include <lib/object-constants.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

extern "C" {
zx_status_t cpp_counter_dispatcher_create(
    ffi::Uninitialized<KernelHandle<CounterDispatcher>>* handle_out);
}

class CounterDispatcher final : public Dispatcher {
 public:
  ~CounterDispatcher() override;

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_COUNTER; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return true; }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final;
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  using Dispatcher::UpdateState;
  using Dispatcher::UpdateStateLocked;

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  friend zx_status_t cpp_counter_dispatcher_create(
      ffi::Uninitialized<KernelHandle<CounterDispatcher>>*);
  CounterDispatcher();

  OpaqueStorage<kCounterDispatcherStateSize, kCounterDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_COUNTER_DISPATCHER_H_
