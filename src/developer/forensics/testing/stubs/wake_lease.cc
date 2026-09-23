// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/wake_lease.h"

#include <lib/async/cpp/wait.h>
#include <lib/stdcompat/vector.h>

namespace forensics::stubs {

namespace {

using fuchsia_power_system::LeaseToken;

}  // namespace

fpromise::promise<LeaseToken, Error> WakeLease::Acquire(const zx::duration timeout) {
  LeaseToken client_token, server_token;
  LeaseToken::create(/*options=*/0u, &client_token, &server_token);

  // Start an async task to wait for EVENTPAIR_PEER_CLOSED signal on server_token.
  zx_handle_t token_handle = server_token.get();
  active_wake_leases_[token_handle] = std::move(server_token);
  auto wait = std::make_unique<async::WaitOnce>(token_handle, ZX_EVENTPAIR_PEER_CLOSED);
  wait->Begin(dispatcher_, [this, token_handle, wait = std::move(wait)](
                               async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                               const zx_packet_signal_t*) {
    FX_CHECK(status == ZX_OK);
    FX_CHECK(active_wake_leases_.contains(token_handle));
    active_wake_leases_.erase(token_handle);
  });

  return fpromise::make_result_promise<LeaseToken, Error>(fpromise::ok(std::move(client_token)));
}

}  // namespace forensics::stubs
