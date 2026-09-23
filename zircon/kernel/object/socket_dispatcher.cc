// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/socket_dispatcher.h"

#include <zircon/errors.h>
#include <zircon/types.h>

extern "C" {
void rust_socket_dispatcher_state_init(void* state, void* holder, uint32_t flags);
void rust_socket_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_socket_dispatcher_state_get_lock(const void* state);
}  // extern "C"

SocketDispatcher::SocketDispatcher(void* holder, uint32_t flags) : Dispatcher(ZX_SOCKET_WRITABLE) {
  DISPATCHER_VERIFY_OFFSET(SocketDispatcher, kSocketDispatcherStateOffset);
  rust_socket_dispatcher_state_init(&opaque_storage_, holder, flags);
}

IMPLEMENT_DISPATCHER_RUST_STATE(SocketDispatcher, rust_socket_dispatcher_state_get_lock,
                                rust_socket_dispatcher_state_destroy)
