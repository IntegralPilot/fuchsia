// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_EVENT_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_EVENT_DISPATCHER_H_

#include <lib/object-constants.h>
#include <sys/types.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class EventDispatcher;

extern "C" {
zx_status_t cpp_event_dispatcher_create(
    uint32_t options, ffi::Uninitialized<KernelHandle<EventDispatcher>>* handle_out);
zx_status_t rust_event_dispatcher_create(
    uint32_t options, ffi::Uninitialized<KernelHandle<EventDispatcher>>* handle_out,
    zx_rights_t* rights_out);
void cpp_event_dispatcher_get_mem_pressure_event(uint32_t kind,
                                                 fbl::RefPtr<EventDispatcher>* out_event);
zx_status_t cpp_memory_stall_event_dispatcher_create(
    uint32_t kind, zx_duration_mono_t threshold, zx_duration_mono_t window,
    ffi::Uninitialized<KernelHandle<EventDispatcher>>* out_handle,
    ffi::Uninitialized<zx_rights_t>* out_rights);
}

class EventDispatcher : public Dispatcher {
 public:
  static zx_status_t Create(uint32_t options,
                            ffi::Uninitialized<KernelHandle<EventDispatcher>>* handle,
                            zx_rights_t* rights) {
    return rust_event_dispatcher_create(options, handle, rights);
  }

  ~EventDispatcher() override;

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_EVENT; }
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

  friend zx_status_t cpp_event_dispatcher_create(
      uint32_t options, ffi::Uninitialized<KernelHandle<EventDispatcher>>* handle_out);
  explicit EventDispatcher(uint32_t options);

 private:
  OpaqueStorage<kEventDispatcherStateSize, kEventDispatcherStateAlign> opaque_storage_;
};

class MemoryStallEventDispatcher final : public EventDispatcher,
                                         private StallObserver::EventReceiver {
 public:
  static zx_status_t Create(zx_system_memory_stall_type_t kind, zx_duration_mono_t threshold,
                            zx_duration_mono_t window, KernelHandle<EventDispatcher>* handle,
                            zx_rights_t* rights);

  ~MemoryStallEventDispatcher() final;

 private:
  explicit MemoryStallEventDispatcher(zx_system_memory_stall_type_t kind);

  void OnAboveThreshold() override;
  void OnBelowThreshold() override;

  // The StallObserver that decides if we are currently above or below the
  // threshold and notifies us through the EventReceiver interface. There is a
  // 1:1 correspondence between instances of StallObserver and
  // MemoryStallEventDispatcher.
  //
  // This would ideally be const (once set, the pointer never changes), but it
  // cannot be because the StallObserver is created after our constructor has
  // already been executed.
  ktl::unique_ptr<StallObserver> observer_;

  // The kind of memory stall that the observer_ has been registered to (used by
  // our destructor to unregister it).
  const zx_system_memory_stall_type_t kind_;
};

fbl::RefPtr<EventDispatcher> GetMemPressureEvent(uint32_t kind);

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_EVENT_DISPATCHER_H_
