// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/public/fakes/crash_reporter.h"

#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>

namespace forensics {
namespace fakes {

using namespace fuchsia::feedback;

void CrashReporter::FileReport(CrashReport report, FileReportCallback callback) {
  if (!report.has_program_name()) {
    callback(fpromise::error(FilingError::INVALID_ARGS_ERROR));
  } else {
    FileReportResults results;
    results.set_result(FilingSuccess::REPORT_UPLOADED);
    results.set_report_id("01234567890abcdef");

    callback(fpromise::ok(std::move(results)));
  }

  ++num_crash_reports_filed_;
  if (querier_) {
    querier_->UpdateAndNotify(num_crash_reports_filed_);
  }
}

void CrashReporter::SetQuerier(
    fidl::InterfaceRequest<fuchsia::feedback::testing::FakeCrashReporterQuerier> request) {
  FX_LOGS(INFO) << "Registering FakeCrashReporterQuerier";
  querier_ = std::make_unique<FakeCrashReporterQuerier>(this, std::move(request),
                                                        num_crash_reports_filed_);
}

void CrashReporter::ResetQuerier() {
  FX_LOGS(INFO) << "Deregistering FakeCrashReporterQuerier";
  querier_.reset();
  // We reset so that the next Querier gets values starting at 0 as they would expect.
  num_crash_reports_filed_ = 0;
}

FakeCrashReporterQuerier::FakeCrashReporterQuerier(
    CrashReporter* crash_reporter,
    fidl::InterfaceRequest<fuchsia::feedback::testing::FakeCrashReporterQuerier> request,
    size_t num_crash_reports_filed)
    : connection_(this, std::move(request)),
      num_crash_reports_filed_(num_crash_reports_filed),
      watch_file_dirty_bit_(true) {
  connection_.set_error_handler(
      [crash_reporter](const zx_status_t status) { crash_reporter->ResetQuerier(); });
}

void FakeCrashReporterQuerier::UpdateAndNotify(size_t num_crash_reports_filed) {
  num_crash_reports_filed_ = num_crash_reports_filed;
  watch_file_dirty_bit_ = true;
  Notify();
}

void FakeCrashReporterQuerier::Notify() {
  if (!callback_.has_value() || !watch_file_dirty_bit_) {
    return;
  }

  (*callback_)(num_crash_reports_filed_);
  callback_ = std::nullopt;
  watch_file_dirty_bit_ = false;
}

void FakeCrashReporterQuerier::WatchFile(WatchFileCallback callback) {
  callback_ = std::move(callback);
  Notify();
}

}  // namespace fakes
}  // namespace forensics
