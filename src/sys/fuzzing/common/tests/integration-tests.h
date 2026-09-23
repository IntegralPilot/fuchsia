// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_TESTS_INTEGRATION_TESTS_H_
#define SRC_SYS_FUZZING_COMMON_TESTS_INTEGRATION_TESTS_H_

#include <fuchsia/fuzzer/cpp/fidl.h>
#include <lib/zx/channel.h>
#include <lib/zx/process.h>
#include <zircon/status.h>

#include <memory>
#include <vector>

#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/child-process.h"
#include "src/sys/fuzzing/common/component-context.h"
#include "src/sys/fuzzing/common/controller-provider.h"
#include "src/sys/fuzzing/common/testing/async-test.h"
#include "src/sys/fuzzing/common/testing/registrar.h"

namespace fuzzing {

using fuchsia::fuzzer::ControllerProviderPtr;
using fuchsia::fuzzer::ControllerPtr;
using fuchsia::fuzzer::Registrar;

// The |EngineIntegrationTest| fakes the registrar but uses a real fuzzing engine.
//
class EngineIntegrationTest : public AsyncTest {
 protected:
  void SetUp() override;

  ComponentContext* context() { return context_.get(); }
  OptionsPtr options() const { return options_; }
  ControllerPtr& controller() { return controller_; }

  // Adds an argument to the command line used to spawn the engine process.
  void AddArg(std::string_view arg);

  // Returns a promise to create a fake registry component and spawn the engine.
  ZxPromise<> StartEngine();

  // Returns a promise to connect to the `fuchsia.fuzzer.Controller`.
  ZxPromise<> Connect();

  // Returns a promise to use the provided `controller` to wait for the engine to produce a
  // non-empty artifact. Returns the artifact and current status when the promise completes via
  // `out_artifact` and `out_status`, respectively.
  ZxPromise<> GetArtifactAndStatus(Artifact* out_artifact, Status* out_status);

  // Returns a promise to wait for the engine process to exit and returns its exit code.
  ZxPromise<int64_t> WaitForEngine();

  void TearDown() override;

  // Integration tests.

  void RunBounded();
  void TryCrashingInput();
  void RunAsTest();

 private:
  ComponentContextPtr context_;
  OptionsPtr options_;
  ControllerPtr controller_;
  std::vector<std::string> cmdline_;
  std::unique_ptr<ChildProcess> engine_;
  ControllerProviderPtr provider_;
  std::unique_ptr<FakeRegistrar> registrar_;
  Scope scope_;
};

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_TESTS_INTEGRATION_TESTS_H_
