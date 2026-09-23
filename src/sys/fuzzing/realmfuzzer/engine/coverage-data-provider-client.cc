// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/realmfuzzer/engine/coverage-data-provider-client.h"

#include <fuchsia/fuzzer/cpp/fidl.h>
#include <lib/sync/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <memory>

#include "src/lib/fxl/macros.h"
#include "src/sys/fuzzing/common/input.h"
#include "src/sys/fuzzing/common/options.h"
#include "src/sys/fuzzing/common/shared-memory.h"

namespace fuzzing {

CoverageDataProviderClient::CoverageDataProviderClient(ExecutorPtr executor)
    : executor_(std::move(executor)), options_(MakeOptions()), receiver_(&sender_) {}

void CoverageDataProviderClient::Configure(const OptionsPtr& options) {
  options_ = options;
  if (provider_.is_bound()) {
    provider_->SetOptions(CopyOptions(*options_));
  }
}

void CoverageDataProviderClient::Bind(RequestHandler handler) {
  FX_CHECK(!provider_.is_bound());
  handler(provider_.NewRequest(executor_->dispatcher()));
  provider_->SetOptions(CopyOptions(*options_));
  // |WatchCoverageData| futures may be abandoned. To prevent coverage data being dropped, a
  // separate future, scoped to this object, should handle the FIDL requests and responses.
  auto task = fpromise::make_promise([this, fut = ZxFuture<std::vector<CoverageData>>()](
                                         Context& context) mutable -> ZxResult<> {
                while (true) {
                  if (!fut) {
                    Bridge<std::vector<CoverageData>> bridge;
                    provider_->WatchCoverageData(bridge.completer.bind());
                    fut = ConsumeBridge(bridge);
                  }
                  if (!fut(context)) {
                    return fpromise::pending();
                  }
                  if (fut.is_error()) {
                    auto status = fut.error();
                    FX_LOGS(WARNING)
                        << "Failed to receive coverage data: " << zx_status_get_string(status);
                    return fpromise::error(status);
                  }
                  for (auto& coverage_data : fut.take_value()) {
                    if (auto status = sender_.Send(std::move(coverage_data)); status != ZX_OK) {
                      FX_LOGS(WARNING) << "Failed to forward received coverage data: "
                                       << zx_status_get_string(status);
                    }
                  }
                }
              }).wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

Promise<CoverageData> CoverageDataProviderClient::GetCoverageData() { return receiver_.Receive(); }

}  // namespace fuzzing
