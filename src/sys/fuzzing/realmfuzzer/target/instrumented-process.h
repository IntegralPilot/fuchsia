// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_REALMFUZZER_TARGET_INSTRUMENTED_PROCESS_H_
#define SRC_SYS_FUZZING_REALMFUZZER_TARGET_INSTRUMENTED_PROCESS_H_

#include <memory>

#include "src/lib/fxl/macros.h"
#include "src/sys/fuzzing/common/component-context.h"
#include "src/sys/fuzzing/realmfuzzer/target/process.h"

namespace fuzzing {

// This class extends |Process| by automatically connecting in a public default constructor. The
// class is instantiated as a singleton below, and lives as long as the process. All other
// fuzzing-related code executed in the target runs as result of the singleton's constructor.
class InstrumentedProcess final {
 public:
  InstrumentedProcess();
  ~InstrumentedProcess() = default;

 private:
  std::unique_ptr<Process> process_;
  std::unique_ptr<ComponentContext> context_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(InstrumentedProcess);
};

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_REALMFUZZER_TARGET_INSTRUMENTED_PROCESS_H_
