// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/suspend_token_dispatcher.h"

#include <lib/object-constants.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>

SuspendTokenDispatcher::SuspendTokenDispatcher() : Dispatcher(0) {
  DISPATCHER_VERIFY_OFFSET(SuspendTokenDispatcher, kSuspendTokenDispatcherStateOffset);
  rust_suspend_token_dispatcher_state_init(&opaque_storage_, this);
}

IMPLEMENT_DISPATCHER_RUST_STATE(SuspendTokenDispatcher,
                                rust_suspend_token_dispatcher_state_get_lock,
                                rust_suspend_token_dispatcher_state_destroy)

void SuspendTokenDispatcher::on_zero_handles() {
  rust_suspend_token_dispatcher_on_zero_handles(this);
}

zx_status_t SuspendTokenDispatcher::user_signal_self(uint32_t clear_mask, uint32_t set_mask) {
  return UserSignalSelfSolo(this, clear_mask, set_mask, 0);
}
