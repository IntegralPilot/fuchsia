// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/exceptions/handler/wake_lease.h"

#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/any_error_in.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>

#include <utility>

#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/promise_timeout.h"
#include "src/lib/fidl/contrib/fpromise/client.h"

namespace forensics::exceptions::handler {
namespace {

namespace fps = fuchsia_power_system;

}  // namespace

WakeLease::WakeLease(async_dispatcher_t* dispatcher, const std::string& lease_name,
                     fidl::ClientEnd<fps::ActivityGovernor> sag_client_end)
    : dispatcher_(dispatcher),
      lease_name_(lease_name),
      sag_(std::move(sag_client_end), dispatcher_, &sag_event_handler_) {}

fpromise::promise<fps::LeaseToken, Error> WakeLease::Acquire(const zx::duration timeout) {
  return MakePromiseTimeout<fps::LeaseToken>(UnsafeAcquire().wrap_with(scope_), dispatcher_,
                                             timeout);
}

fpromise::promise<fps::LeaseToken, Error> WakeLease::UnsafeAcquire() {
  return fidl_fpromise::as_promise(sag_->AcquireWakeLease(lease_name_))
      .or_else([](const fidl::ErrorsIn<fps::ActivityGovernor::AcquireWakeLease>& error) {
        FX_LOGS(ERROR) << "Failed to acquire wake lease: " << error.FormatDescription();
        return fpromise::make_result_promise<fps::ActivityGovernorAcquireWakeLeaseResponse, Error>(
            fpromise::error(Error::kBadValue));
      })
      .and_then([](fps::ActivityGovernorAcquireWakeLeaseResponse& response) mutable {
        return fpromise::make_result_promise<fps::LeaseToken, Error>(
            fpromise::ok(std::move(response.token())));
      });
}

}  // namespace forensics::exceptions::handler
