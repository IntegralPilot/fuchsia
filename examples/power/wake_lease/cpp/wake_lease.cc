// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/power/wake_lease/cpp/wake_lease.h"

#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>

#include <string>

namespace examples::power {

fpromise::promise<WakeLease, Error> WakeLease::Take(
    const fidl::Client<fuchsia_power_system::ActivityGovernor>& client, const std::string& name) {
  fpromise::bridge<WakeLease, Error> bridge;
  client->AcquireWakeLease({name}).Then(
      [completer = std::move(bridge.completer)](auto& result) mutable {
        if (result.is_error()) {
          completer.complete_error("Failed to acquire wake lease");
          return;
        }
        completer.complete_ok(WakeLease(std::move(result.value().token())));
      });
  return bridge.consumer.promise();
}

}  // namespace examples::power
