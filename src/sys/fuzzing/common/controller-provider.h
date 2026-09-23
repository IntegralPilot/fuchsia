// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_CONTROLLER_PROVIDER_H_
#define SRC_SYS_FUZZING_COMMON_CONTROLLER_PROVIDER_H_

#include <fuchsia/fuzzer/cpp/fidl.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/zx/channel.h>

#include <string>

#include "src/lib/fxl/macros.h"
#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/controller.h"
#include "src/sys/fuzzing/common/runner.h"

namespace fuzzing {

using ::fuchsia::fuzzer::Controller;
using ::fuchsia::fuzzer::ControllerProvider;
using ::fuchsia::fuzzer::RegistrarPtr;

class ControllerProviderImpl final : public ControllerProvider {
 public:
  explicit ControllerProviderImpl(RunnerPtr runner);
  ~ControllerProviderImpl() override = default;

  // FIDL methods.
  void Connect(fidl::InterfaceRequest<Controller> request, ConnectCallback callback) override;
  void Stop() override;

  // Returns a promise to register a controller for the given `url`, using the given `channel` to
  // the fuzz-registry.
  Promise<> Serve(const std::string& url, zx::channel channel);

 private:
  fidl::Binding<ControllerProvider> binding_;
  ControllerImpl controller_;
  RegistrarPtr registrar_;
  Scope scope_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(ControllerProviderImpl);
};

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_CONTROLLER_PROVIDER_H_
