// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_ENGINE_H_
#define SRC_SYS_FUZZING_COMMON_ENGINE_H_

#include <string>
#include <vector>

#include "src/lib/pkg_url/fuchsia_pkg_url.h"
#include "src/sys/fuzzing/common/component-context.h"
#include "src/sys/fuzzing/common/runner.h"

namespace fuzzing {

// The `Engine` represents a generic fuzzing engine. Specific engines with specific runners should
// call `RunEngine` with command line arguments and a `Runner` factory method.
class Engine {
 public:
  Engine();
  virtual ~Engine() = default;

  ComponentContext& context() const { return *context_; }
  void set_pkg_dir(const std::string& pkg_dir) { pkg_dir_ = pkg_dir; }

  // Runs the engine. The given `runner` must not be null.
  __WARN_UNUSED_RESULT zx_status_t Run(int argc, char** argv, RunnerPtr runner);

 protected:
  Engine(ComponentContextPtr context, const std::string& pkg_dir);

  const std::vector<std::string>& args() const { return args_; }
  bool fuzzing() const { return fuzzing_; }
  std::string url() const { return url_.ToString(); }

  zx_status_t Initialize(std::vector<std::string> args);

 private:
  // Runs the engine in "fuzzing" mode: the engine will serve `fuchsia.fuzzer.ControllerProvider`
  // and fulfill `fuchsia.fuzzer.Controller` requests.
  __WARN_UNUSED_RESULT zx_status_t RunFuzzer();

  // Runs the engine in "test" mode: the engine will execute the fuzzer with the set of inputs
  // given by seed corpora listed in the fuzzer's command line arguments.
  __WARN_UNUSED_RESULT zx_status_t RunTest();

  ComponentContextPtr context_;
  std::vector<std::string> args_;
  std::string pkg_dir_;
  component::FuchsiaPkgUrl url_;
  RunnerPtr runner_;
  bool fuzzing_ = false;
  Input dictionary_;
};

// Starts the engine with runner provided by `MakeRunnerPtr`, which should have the signature:
// `ZxResult<RunnerPtr>(ComponentContext&)`.
template <typename RunnerPtrMaker>
zx_status_t RunEngine(int argc, char** argv, RunnerPtrMaker MakeRunnerPtr) {
  Engine engine;
  auto runner = MakeRunnerPtr(engine.context());
  if (runner.is_error()) {
    return runner.error();
  }
  return engine.Run(argc, argv, runner.take_value());
}

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_ENGINE_H_
