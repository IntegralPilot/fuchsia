// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/io_buffer_dispatcher.h"

#include <object/handle.h>

extern "C" {
void rust_io_buffer_dispatcher_state_init(void* state, void* holder, size_t endpoint_id,
                                          void* shared_state);
void rust_io_buffer_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_io_buffer_dispatcher_state_get_lock(const void* state);
}  // extern "C"

IoBufferDispatcher::IoBufferDispatcher(void* holder, IobEndpointId endpoint_id, void* shared_state)
    : Dispatcher(0) {
  DISPATCHER_VERIFY_OFFSET(IoBufferDispatcher, kIoBufferDispatcherStateOffset);
  rust_io_buffer_dispatcher_state_init(&opaque_storage_, holder, static_cast<size_t>(endpoint_id),
                                       shared_state);
}

IMPLEMENT_DISPATCHER_RUST_STATE(IoBufferDispatcher, rust_io_buffer_dispatcher_state_get_lock,
                                rust_io_buffer_dispatcher_state_destroy)
