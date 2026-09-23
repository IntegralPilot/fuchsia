// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_TIMER_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_TIMER_DISPATCHER_H_

#include <lib/object-constants.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <kernel/dpc.h>
#include <kernel/ffi.h>
#include <kernel/timer.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class TimerDispatcher;

extern "C" {

zx_status_t cpp_timer_dispatcher_create(
    uint32_t options, zx_clock_t clock_id,
    ffi::Uninitialized<KernelHandle<TimerDispatcher>>* handle_out);
void cpp_timer_dispatcher_init_dpc(void* dpc_storage, const TimerDispatcher* disp);
void timer_irq_callback(Timer* timer, zx_time_t now, void* arg);

void rust_timer_dispatcher_state_init(void* state, void* disp, uint32_t options,
                                      zx_clock_t clock_id);
void rust_timer_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_timer_dispatcher_state_get_lock(const void* state);
void rust_timer_dispatcher_on_zero_handles(const TimerDispatcher* disp);
void rust_timer_dispatcher_on_timer_fired(const TimerDispatcher* disp);
zx_info_timer_t rust_timer_dispatcher_get_info(const TimerDispatcher* disp);

}  // extern "C"

class TimerDispatcher final : public Dispatcher {
 public:
  ~TimerDispatcher() final;
  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_TIMER; }
  void on_zero_handles() final { rust_timer_dispatcher_on_zero_handles(this); }
  bool is_waitable() const final { return true; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final {
    return UserSignalSelfSolo(this, clear_mask, set_mask, 0);
  }
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Timer callback.
  void OnTimerFired();

  zx_info_timer_t GetInfo() const;

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  friend zx_status_t cpp_timer_dispatcher_create(
      uint32_t options, zx_clock_t clock_id,
      ffi::Uninitialized<KernelHandle<TimerDispatcher>>* handle_out);
  TimerDispatcher(uint32_t options, zx_clock_t clock_id);

  OpaqueStorage<kTimerDispatcherStateSize, kTimerDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_TIMER_DISPATCHER_H_
