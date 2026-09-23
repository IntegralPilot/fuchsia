// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_EVENT_PAIR_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_EVENT_PAIR_DISPATCHER_H_

#include <lib/object-constants.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class EventPairDispatcher;

DECLARE_PEERED_DISPATCHER_RUST_PROTOS(EventPairDispatcher, rust_event_pair_dispatcher)

extern "C" {
zx_status_t cpp_event_pair_dispatcher_create(
    void* holder, ffi::Uninitialized<KernelHandle<EventPairDispatcher>>* handle_out);
zx_status_t rust_event_pair_dispatcher_create(KernelHandle<EventPairDispatcher>* handle0_out,
                                              KernelHandle<EventPairDispatcher>* handle1_out,
                                              zx_rights_t* rights_out);
}  // extern "C"

class EventPairDispatcher final : public Dispatcher {
 public:
  explicit EventPairDispatcher(void* holder);
  ~EventPairDispatcher() final;

  DECLARE_PEERED_DISPATCHER_RUST_METHODS(rust_event_pair_dispatcher, ZX_OBJ_TYPE_EVENTPAIR, true)

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  OpaqueStorage<kEventPairDispatcherStateSize, kEventPairDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_EVENT_PAIR_DISPATCHER_H_
