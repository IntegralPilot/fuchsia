// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/engine.h"
#include "src/sys/fuzzing/realmfuzzer/engine/runner.h"

namespace fuzzing {

ZxResult<RunnerPtr> MakeRealmFuzzerRunnerPtr(ComponentContext& context) {
  auto runner = RealmFuzzerRunner::MakePtr(context.executor());
  auto runner_impl = std::static_pointer_cast<RealmFuzzerRunner>(runner);
  runner_impl->SetAdapterHandler(context.MakeRequestHandler<TargetAdapter>());
  runner_impl->SetProviderHandler(context.MakeRequestHandler<CoverageDataProvider>());
  return fpromise::ok(std::move(runner));
}

}  // namespace fuzzing

int main(int argc, char** argv) {
  return fuzzing::RunEngine(argc, argv, fuzzing::MakeRealmFuzzerRunnerPtr);
}
