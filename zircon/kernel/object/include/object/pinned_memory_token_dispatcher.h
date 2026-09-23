// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PINNED_MEMORY_TOKEN_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PINNED_MEMORY_TOKEN_DISPATCHER_H_

#include <lib/object-constants.h>
#include <sys/types.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <kernel/lockdep.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class BusTransactionInitiatorDispatcher;
class PinnedMemoryTokenDispatcher;

extern "C" {
zx_status_t cpp_pinned_memory_token_dispatcher_create(
    BusTransactionInitiatorDispatcher* bti,
    ffi::Uninitialized<KernelHandle<PinnedMemoryTokenDispatcher>>* handle_out);

void rust_pinned_memory_token_dispatcher_state_init(void* state, void* disp,
                                                    BusTransactionInitiatorDispatcher* bti);
void rust_pinned_memory_token_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_pinned_memory_token_dispatcher_state_get_lock(const void* state);
void rust_pinned_memory_token_dispatcher_on_zero_handles(const PinnedMemoryTokenDispatcher* disp);
}

class PinnedMemoryTokenDispatcher final : public Dispatcher {
 public:
  ~PinnedMemoryTokenDispatcher() final;

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_PMT; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return false; }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final {
    return UserSignalSelfSolo(this, clear_mask, set_mask, 0);
  }
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  void on_zero_handles() final;

 protected:
  friend BusTransactionInitiatorDispatcher;
  Lock<CriticalMutex>* get_lock() const final;

 private:
  friend zx_status_t cpp_pinned_memory_token_dispatcher_create(
      BusTransactionInitiatorDispatcher* bti,
      ffi::Uninitialized<KernelHandle<PinnedMemoryTokenDispatcher>>* handle_out);

  explicit PinnedMemoryTokenDispatcher(fbl::RefPtr<BusTransactionInitiatorDispatcher> bti);
  DISALLOW_COPY_ASSIGN_AND_MOVE(PinnedMemoryTokenDispatcher);

  OpaqueStorage<kPinnedMemoryTokenDispatcherStateSize, kPinnedMemoryTokenDispatcherStateAlign>
      opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PINNED_MEMORY_TOKEN_DISPATCHER_H_
