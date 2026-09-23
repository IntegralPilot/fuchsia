// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/log_dispatcher.h"

#include <zircon/errors.h>

#include <ktl/enforce.h>

extern "C" {
void rust_log_dispatcher_state_init(void* state, void* disp, uint32_t flags);
void rust_log_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_log_dispatcher_state_get_lock(const void* state);
}  // extern "C"

LogDispatcher::LogDispatcher(uint32_t flags) : Dispatcher(ZX_LOG_WRITABLE) {
  DISPATCHER_VERIFY_OFFSET(LogDispatcher, kLogDispatcherStateOffset);
  rust_log_dispatcher_state_init(&opaque_storage_, this, flags);
}

IMPLEMENT_DISPATCHER_RUST_STATE(LogDispatcher, rust_log_dispatcher_state_get_lock,
                                rust_log_dispatcher_state_destroy)

zx_status_t LogDispatcher::user_signal_self(uint32_t clear_mask, uint32_t set_mask) {
  return UserSignalSelfSolo(this, clear_mask, set_mask, 0);
}
