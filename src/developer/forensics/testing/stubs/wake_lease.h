// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_WAKE_LEASE_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_WAKE_LEASE_H_

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fpromise/promise.h>

#include <unordered_map>

#include "src/developer/forensics/exceptions/handler/wake_lease.h"
#include "src/developer/forensics/utils/errors.h"

namespace forensics::stubs {

class WakeLease : public exceptions::handler::WakeLeaseBase {
 public:
  explicit WakeLease(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  fpromise::promise<fuchsia_power_system::LeaseToken, Error> Acquire(zx::duration timeout) override;

  bool LeaseHeld() const { return active_wake_leases_.size() > 0; }

 private:
  async_dispatcher_t* dispatcher_;
  std::unordered_map<zx_handle_t, fuchsia_power_system::LeaseToken> active_wake_leases_;
};

}  // namespace forensics::stubs

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_WAKE_LEASE_H_
