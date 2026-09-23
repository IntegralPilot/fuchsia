// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYS_COMPONENT_CPP_TESTING_EXECUTION_CONTROLLER_H_
#define LIB_SYS_COMPONENT_CPP_TESTING_EXECUTION_CONTROLLER_H_

#include <fuchsia/component/cpp/fidl.h>
#include <lib/fit/result.h>

namespace component_testing {

// A controller used to influence and observe a specific execution of a
// component. The component will be stopped when this is destroyed if it is
// still running from the `Start` call that created this controller. If the
// component has already stopped, or even been restarted by some other action,
// then dropping this will do nothing.
class ExecutionController final {
 public:
  explicit ExecutionController(
      fuchsia::component::ExecutionControllerPtr execution_controller_proxy)
      : execution_controller_proxy_(std::move(execution_controller_proxy)) {}

  // Initiates a stop action if the component is not already in the process of
  // or has finished stopping.
  void Stop() { execution_controller_proxy_->Stop(); }

  using StopCallback = fit::function<void(fuchsia::component::StoppedPayload)>;

  // Sets a callback to be invoked when the current execution of the component
  // has stopped.
  void OnStop(StopCallback callback) {
    execution_controller_proxy_.events().OnStop = std::move(callback);
  }

 private:
  fuchsia::component::ExecutionControllerPtr execution_controller_proxy_;
};

}  // namespace component_testing

#endif  // LIB_SYS_COMPONENT_CPP_TESTING_EXECUTION_CONTROLLER_H_
