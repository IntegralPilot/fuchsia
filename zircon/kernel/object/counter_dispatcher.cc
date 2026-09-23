// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/counter_dispatcher.h"

#include <zircon/errors.h>
#include <zircon/rights.h>

#include <new>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <ktl/utility.h>
#include <object/handle.h>
#include <object/process_dispatcher.h>

#include <ktl/enforce.h>

extern "C" {
void rust_counter_dispatcher_state_init(void* state, void* disp);
void rust_counter_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_counter_dispatcher_state_get_lock(const void* state);
}  // extern "C"

CounterDispatcher::CounterDispatcher() : Dispatcher(ZX_COUNTER_NON_POSITIVE) {
  DISPATCHER_VERIFY_OFFSET(CounterDispatcher, kCounterDispatcherStateOffset);
  rust_counter_dispatcher_state_init(&opaque_storage_, this);
}

IMPLEMENT_DISPATCHER_RUST_STATE(CounterDispatcher, rust_counter_dispatcher_state_get_lock,
                                rust_counter_dispatcher_state_destroy)

zx_status_t CounterDispatcher::user_signal_self(uint32_t clear_mask, uint32_t set_mask) {
  return UserSignalSelfSolo(this, clear_mask, set_mask, ZX_COUNTER_SIGNALED);
}
