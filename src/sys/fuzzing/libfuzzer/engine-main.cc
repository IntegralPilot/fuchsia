// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <string>
#include <vector>

#include "src/sys/fuzzing/common/engine.h"
#include "src/sys/fuzzing/libfuzzer/runner.h"

namespace fuzzing {

ZxResult<RunnerPtr> MakeLibFuzzerRunnerPtr(ComponentContext& context) {
  return fpromise::ok(LibFuzzerRunner::MakePtr(context.executor()));
}

}  // namespace fuzzing

int main(int argc, char** argv) {
  return fuzzing::RunEngine(argc, argv, fuzzing::MakeLibFuzzerRunnerPtr);
}
