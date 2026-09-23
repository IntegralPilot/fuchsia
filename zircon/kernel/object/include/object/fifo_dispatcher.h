// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_FIFO_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_FIFO_DISPATCHER_H_

#include <lib/object-constants.h>
#include <stdint.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class FifoDispatcher;

DECLARE_PEERED_DISPATCHER_RUST_PROTOS(FifoDispatcher, rust_fifo_dispatcher)

extern "C" {
zx_status_t cpp_fifo_dispatcher_create(
    void* holder, uint32_t count, uint32_t elem_size, void* data,
    ffi::Uninitialized<KernelHandle<FifoDispatcher>>* handle_out);
}  // extern "C"

class FifoDispatcher final : public Dispatcher {
 public:
  FifoDispatcher(void* holder, uint32_t count, uint32_t elem_size, void* data);
  ~FifoDispatcher() final;

  DECLARE_PEERED_DISPATCHER_RUST_METHODS(rust_fifo_dispatcher, ZX_OBJ_TYPE_FIFO, true)

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  OpaqueStorage<kFifoDispatcherStateSize, kFifoDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_FIFO_DISPATCHER_H_
