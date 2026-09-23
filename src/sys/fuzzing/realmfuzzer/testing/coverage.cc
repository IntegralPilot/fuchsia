// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/realmfuzzer/testing/coverage.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

namespace fuzzing {

using fuchsia::fuzzer::Data;
using fuchsia::fuzzer::InstrumentedProcess;

FakeCoverage::FakeCoverage(ExecutorPtr executor)
    : collector_(this),
      provider_(this),
      executor_(executor),
      options_(MakeOptions()),
      receiver_(&sender_) {
  auto self = zx::process::self();
  zx_info_handle_basic_t info;
  auto status = self->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  FX_CHECK(status == ZX_OK) << zx_status_get_string(status);
  target_id_ = info.koid;
}

fidl::InterfaceRequestHandler<CoverageDataCollector> FakeCoverage::GetCollectorHandler() {
  return [this](fidl::InterfaceRequest<CoverageDataCollector> request) {
    if (auto status = collector_.Bind(request.TakeChannel(), executor_->dispatcher());
        status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to bind fuchsia.fuzzer.CoverageDataCollector request: "
                     << zx_status_get_string(status);
    }
  };
}

fidl::InterfaceRequestHandler<CoverageDataProvider> FakeCoverage::GetProviderHandler() {
  return [this](fidl::InterfaceRequest<CoverageDataProvider> request) {
    if (auto status = provider_.Bind(std::move(request), executor_->dispatcher());
        status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to bind fuchsia.fuzzer.CoverageDataProvider request: "
                     << zx_status_get_string(status);
    }
  };
}

void FakeCoverage::Initialize(zx::eventpair eventpair, zx::process process,
                              InitializeCallback callback) {
  CoverageData coverage_data = {
      .target_id = target_id_,
      .data = Data::WithInstrumented({
          .eventpair = std::move(eventpair),
          .process = std::move(process),
      }),
  };
  if (auto status = sender_.Send(std::move(coverage_data)); status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to send instrumented process to provider: "
                   << zx_status_get_string(status);
    return;
  }
  callback(CopyOptions(*options_));
}

void FakeCoverage::AddInline8bitCounters(zx::vmo inline_8bit_counters,
                                         AddInline8bitCountersCallback callback) {
  CoverageData coverage_data = {
      .target_id = target_id_,
      .data = Data::WithInline8bitCounters(std::move(inline_8bit_counters)),
  };
  if (auto status = sender_.Send(std::move(coverage_data)); status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to send inline 8-bit counters to provider: "
                   << zx_status_get_string(status);
    return;
  }
  callback();
}

void FakeCoverage::SetOptions(Options options) { ::fuzzing::SetOptions(options_.get(), options); }

void FakeCoverage::WatchCoverageData(WatchCoverageDataCallback callback) {
  auto task =
      fpromise::make_promise([this, callback = std::move(callback),
                              coverage_data = std::vector<CoverageData>(),
                              fut = Future<CoverageData>()](Context& context) mutable -> Result<> {
        while (true) {
          if (!fut) {
            while (true) {
              auto result = receiver_.TryReceive();
              if (result.is_error()) {
                break;
              }
              coverage_data.emplace_back(result.take_value());
            }
            if (first_ || !coverage_data.empty()) {
              callback(std::move(coverage_data));
              first_ = false;
              return fpromise::ok();
            }
            fut = receiver_.Receive();
          }
          if (!fut(context)) {
            return fpromise::pending();
          }
          if (fut.is_error()) {
            FX_LOGS(WARNING) << "Failed to receive coverage data";
            return fpromise::error();
          }
          coverage_data.emplace_back(fut.take_value());
        }
      }).wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void FakeCoverage::Send(CoverageData coverage_data) {
  auto status = sender_.Send(std::move(coverage_data));
  FX_CHECK(status == ZX_OK) << zx_status_get_string(status);
}

Result<CoverageData> FakeCoverage::TryReceive() { return receiver_.TryReceive(); }

Promise<CoverageData> FakeCoverage::Receive() { return receiver_.Receive(); }

}  // namespace fuzzing
