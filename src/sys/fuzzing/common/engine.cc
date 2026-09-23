// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/engine.h"

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/sys/fuzzing/common/controller-provider.h"

namespace fuzzing {

using ::fuchsia::fuzzer::FUZZ_MODE;

Engine::Engine() : Engine(ComponentContext::Create(), "/pkg") {}

Engine::Engine(ComponentContextPtr context, const std::string& pkg_dir)
    : context_(std::move(context)), pkg_dir_(pkg_dir) {
  FX_CHECK(context_);
}

zx_status_t Engine::Run(int argc, char** argv, RunnerPtr runner) {
  FX_CHECK(runner);
  fuzzing_ = false;
  dictionary_ = Input();

  // Skip the process name.
  std::vector<std::string> args(argv + 1, argv + argc);
  if (auto status = Initialize(std::move(args)); status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to initialize engine: " << zx_status_get_string(status);
    return status;
  }
  runner_ = runner;
  return fuzzing_ ? RunFuzzer() : RunTest();
}

zx_status_t Engine::Initialize(std::vector<std::string> args) {
  args_ = std::move(args);
  fuzzing_ = false;
  std::string url;
  bool skip = false;
  auto is_engine_flag = [this, &url, &skip](const std::string& arg) {
    if (skip) {
      return false;
    }
    if (arg == "--") {
      skip = true;
      return false;
    }
    // Look for the fuzzing indicator. This is typically provided by `fuzz-manager`.
    if (arg == FUZZ_MODE) {
      fuzzing_ = true;
      return true;
    }
    if (arg[0] != '-' && url.empty()) {
      url = arg;
      return true;
    }
    return false;
  };
  args_.erase(std::remove_if(args_.begin(), args_.end(), is_engine_flag), args_.end());
  if (url.empty()) {
    FX_LOGS(ERROR) << "Fuzzer URL not provided by component runner";
    return ZX_ERR_INVALID_ARGS;
  }
  if (!url_.Parse(url)) {
    FX_LOGS(ERROR) << "Failed to parse URL: " << url;
    return ZX_ERR_INVALID_ARGS;
  }
  return ZX_OK;
}

zx_status_t Engine::RunFuzzer() {
  // No need to `wrap_with`. Everything remains in scope as long as context_->Run() is executing.
  ControllerProviderImpl provider(runner_);
  auto init = runner_->Initialize(pkg_dir_, args_).or_else([](zx_status_t& status) -> Result<> {
    FX_LOGS(ERROR) << "Failed to initialize fuzzer: " << zx_status_get_string(status);
    return fpromise::error();
  });
  auto serve = provider.Serve(url_.ToString(), context_->TakeChannel(0));
  context_->ScheduleTask(init.and_then(std::move(serve)));
  return context_->Run();
}

zx_status_t Engine::RunTest() {
  // TODO(https://fxbug.dev/42060461): Rarely, spawned process output may be truncated. `LibFuzzerRunner`
  // needs to return `ZX_ERR_IO_INVALID` in this case. By retying several times, the probability of
  // the underlying flake failing a test drops to almost zero.
  static constexpr const size_t kFuzzerTestRetries = 10;

  // In order to make this more testable, the following task does not exit directly. Instead, it
  // repeatedly calls |RunUntilIdle| until it has set an exit code. This allows this method to be
  // called as part of a gTest as well as by the elf_test_runner.
  zx_status_t exitcode = ZX_ERR_NEXT;
  auto task = runner_->Initialize(pkg_dir_, args_)
                  .and_then([this, fut = ZxFuture<Artifact>(),
                             attempts = 0U](Context& context) mutable -> ZxResult<Artifact> {
                    while (attempts < kFuzzerTestRetries) {
                      if (!fut) {
                        auto corpus = runner_->GetCorpus(CorpusType::SEED);
                        corpus.emplace_back(Input());
                        FX_LOGS(INFO) << "Testing with " << corpus.size() << " input(s).";
                        fut = runner_->TryEach(std::move(corpus));
                      }
                      if (!fut(context)) {
                        return fpromise::pending();
                      }
                      if (fut.is_ok()) {
                        return fpromise::ok(fut.take_value());
                      }
                      if (auto status = fut.take_error(); status != ZX_ERR_IO_INVALID) {
                        return fpromise::error(status);
                      }
                      attempts++;
                    }
                    return fpromise::error(ZX_ERR_IO_INVALID);
                  })
                  .then([&exitcode](ZxResult<Artifact>& result) {
                    if (result.is_error()) {
                      exitcode = result.error();
                      return;
                    }
                    auto artifact = result.take_value();
                    auto fuzz_result = artifact.fuzz_result();
                    exitcode = (fuzz_result == FuzzResult::NO_ERRORS) ? 0 : fuzz_result;
                  });
  context_->ScheduleTask(std::move(task));
  while (exitcode == ZX_ERR_NEXT) {
    if (auto status = context_->RunUntilIdle(); status != ZX_OK) {
      return status;
    }
  }
  return exitcode;
}

}  // namespace fuzzing
