// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_STOP_SIGNALS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_STOP_SIGNALS_H_

#include <fuchsia/hardware/power/statecontrol/cpp/fidl.h>
#include <fuchsia/process/lifecycle/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <lib/zx/channel.h>

#include "src/developer/forensics/feedback/reboot_log/graceful_shutdown_info.h"
#include "src/developer/forensics/utils/errors.h"

namespace forensics::feedback {

// Indicates `fuchsia.process.lifecycle/Lifecycle.Stop` has been called and provides a way to
// sends a response to the server.
class LifecycleStopSignal {
 public:
  explicit LifecycleStopSignal(fit::callback<void(void)> callback);

  void Respond() { callback_(); }

 private:
  fit::callback<void(void)> callback_;
};

// Indicates `fuchsia.hardware.power.statecontrol/ShutdownWatcher.OnShutdown` has been called and
// provides a way to get info about the shutdown and send a response to the server.
class GracefulShutdownInfoSignal {
 public:
  GracefulShutdownInfoSignal(GracefulShutdownAction action,
                             std::vector<GracefulShutdownReason> reasons,
                             fit::callback<void(void)> callback);

  GracefulShutdownAction Action() const { return action_; }
  std::vector<GracefulShutdownReason> Reasons() const { return reasons_; }
  void Respond() { callback_(); }

 private:
  GracefulShutdownAction action_;
  std::vector<GracefulShutdownReason> reasons_;
  fit::callback<void(void)> callback_;
};

// Returns a promise which will complete successfully when the lifecycle signal is received.
//
// Note, the response will be sent when the `LifecycleStopSignal` object is destroyed, if it hasn't
// already been sent.
fpromise::promise<LifecycleStopSignal, Error> WaitForLifecycleStop(
    async_dispatcher_t* dispatcher,
    fidl::InterfaceRequest<fuchsia::process::lifecycle::Lifecycle> request);

// Returns a promise which will complete successfully when the shutdown signal is received.
//
// Note, the response will be sent when the `GracefulShutdownInfoSignal` object is destroyed, if it
// hasn't already been sent.
fpromise::promise<GracefulShutdownInfoSignal, Error> WaitForShutdownInfo(
    async_dispatcher_t* dispatcher,
    fidl::InterfaceRequest<fuchsia::hardware::power::statecontrol::ShutdownWatcher> request);

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_STOP_SIGNALS_H_
