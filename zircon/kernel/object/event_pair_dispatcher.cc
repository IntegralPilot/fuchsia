// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/event_pair_dispatcher.h"

extern "C" {
void rust_event_pair_dispatcher_state_init(void* state, void* holder);
void rust_event_pair_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_event_pair_dispatcher_state_get_lock(const void* state);
}  // extern "C"

EventPairDispatcher::EventPairDispatcher(void* holder) : Dispatcher(0u) {
  DISPATCHER_VERIFY_OFFSET(EventPairDispatcher, kEventPairDispatcherStateOffset);
  rust_event_pair_dispatcher_state_init(&opaque_storage_, holder);
}

IMPLEMENT_DISPATCHER_RUST_STATE(EventPairDispatcher, rust_event_pair_dispatcher_state_get_lock,
                                rust_event_pair_dispatcher_state_destroy)
