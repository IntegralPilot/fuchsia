// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/fifo_dispatcher.h"

extern "C" {
void rust_fifo_dispatcher_state_init(void* state, void* holder, uint32_t count, uint32_t elem_size,
                                     void* data);
void rust_fifo_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_fifo_dispatcher_state_get_lock(const void* state);
}  // extern "C"

FifoDispatcher::FifoDispatcher(void* holder, uint32_t count, uint32_t elem_size, void* data)
    : Dispatcher(ZX_FIFO_WRITABLE) {
  DISPATCHER_VERIFY_OFFSET(FifoDispatcher, kFifoDispatcherStateOffset);
  rust_fifo_dispatcher_state_init(&opaque_storage_, holder, count, elem_size, data);
}

IMPLEMENT_DISPATCHER_RUST_STATE(FifoDispatcher, rust_fifo_dispatcher_state_get_lock,
                                rust_fifo_dispatcher_state_destroy)
