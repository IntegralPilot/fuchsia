// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_EXCEPTIONS_HANDLER_WAKE_LEASE_H_
#define SRC_DEVELOPER_FORENSICS_EXCEPTIONS_HANDLER_WAKE_LEASE_H_

#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>

#include <string>

#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/fidl_event_handler.h"

namespace forensics::exceptions::handler {

class WakeLeaseBase {
 public:
  virtual ~WakeLeaseBase() = default;

  virtual fpromise::promise<fuchsia_power_system::LeaseToken, Error> Acquire(
      zx::duration timeout) = 0;
};

// Acquires a wake lease to prevent the system from suspending.
class WakeLease : public WakeLeaseBase {
 public:
  WakeLease(async_dispatcher_t* dispatcher, const std::string& lease_name,
            fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag_client_end);

  // The promise returned needs to be scheduled on an executor and will complete ok with the power
  // lease token if successful. If there is an error, the promise will return an error indicating
  // why, e.g., the timeout has been hit and the lease couldn't be acquired.
  //
  // This function can be called many times. If the lease returned falls out of scope, the lease
  // will be dropped and another can be later reacquired.
  fpromise::promise<fuchsia_power_system::LeaseToken, Error> Acquire(zx::duration timeout) override;

 private:
  // "Unsafe" because it does not have scoping to prevent |this| from being accessed in promise
  // continutations.
  fpromise::promise<fuchsia_power_system::LeaseToken, Error> UnsafeAcquire();

  async_dispatcher_t* dispatcher_;
  std::string lease_name_;

  AsyncEventHandlerOpen<fuchsia_power_system::ActivityGovernor> sag_event_handler_;
  fidl::Client<fuchsia_power_system::ActivityGovernor> sag_;

  // Enables this to be safely captured in promises returned by Acquire. Any public method that
  // returns a promise must wrap it with |scope_|.
  fpromise::scope scope_;
};

}  // namespace forensics::exceptions::handler

#endif  // SRC_DEVELOPER_FORENSICS_EXCEPTIONS_HANDLER_WAKE_LEASE_H_
