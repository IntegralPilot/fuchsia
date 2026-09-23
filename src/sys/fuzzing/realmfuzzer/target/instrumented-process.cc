// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/realmfuzzer/target/instrumented-process.h"

#include <lib/zx/eventpair.h>
#include <zircon/time.h>

#include <memory>

#include "src/lib/fxl/macros.h"
#include "src/sys/fuzzing/common/component-context.h"
#include "src/sys/fuzzing/common/sancov.h"
#include "src/sys/fuzzing/realmfuzzer/target/process.h"

namespace fuzzing {

using fuchsia::fuzzer::CoverageDataCollector;

NO_SANITIZE_ALL
InstrumentedProcess::InstrumentedProcess() {
  context_ = ComponentContext::CreateAuxillary();
  process_ = std::make_unique<Process>(context_->executor());
  Process::InstallHooks();
  fidl::InterfaceHandle<CoverageDataCollector> collector;
  if (auto status = context_->Connect(collector.NewRequest()); status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to connect to coverage data collector: "
                   << zx_status_get_string(status);
  }
  zx::eventpair send, recv;
  if (auto status = zx::eventpair::create(0, &send, &recv); status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to create eventpair: " << zx_status_get_string(status);
  }
  context_->ScheduleTask(process_->Connect(std::move(collector), std::move(send)));
  if (auto status = context_->Run(); status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to start component context: " << zx_status_get_string(status);
  }
  if (auto status = recv.wait_one(kSync | ZX_EVENTPAIR_PEER_CLOSED, zx::time::infinite(), nullptr);
      status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to wait on eventpair: " << zx_status_get_string(status);
  }
}

// The weakly linked symbols should be examined as late as possible, in order to guarantee all of
// the module constructors execute first. To achieve this, the singleton's constructor uses a
// priority attribute to ensure it is run just before |main|.
[[gnu::init_priority(0xffff)]] InstrumentedProcess gInstrumented;

}  // namespace fuzzing
