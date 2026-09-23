// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/scenic/util/error_reporter.h"

#include <memory>

namespace scenic_impl {

namespace {

#define LOG_HELPER(severity) FX_LOGS(severity) << error_string;

class DefaultErrorReporter : public ErrorReporter {
 public:
  void ReportError(fuchsia_logging::LogSeverity severity, std::string error_string) override {
    switch (severity) {
      case fuchsia_logging::LogSeverity::Info:
        FX_LOGS(INFO) << error_string;
        break;
      case fuchsia_logging::LogSeverity::Warn:
        FX_LOGS(WARNING) << error_string;
        break;
      case fuchsia_logging::LogSeverity::Error:
        FX_LOGS(ERROR) << error_string;
        break;
      case fuchsia_logging::LogSeverity::Fatal:
        FX_LOGS(FATAL) << error_string;
        break;
      default:
        // Invalid severity.
        FX_DCHECK(false);
    }
  }
};

}  // anonymous namespace

ErrorReporter::Report::Report(ErrorReporter* owner, fuchsia_logging::LogSeverity severity,
                              const std::string& prefix)
    : owner_(owner), severity_(severity) {
  stream_ << prefix;
}

ErrorReporter::Report::Report(Report&& other)
    : owner_(other.owner_), severity_(other.severity_), stream_(std::move(other.stream_)) {}

ErrorReporter::Report::~Report() { owner_->ReportError(severity_, stream_.str()); }

const std::shared_ptr<ErrorReporter>& ErrorReporter::Default() {
  static const std::shared_ptr<ErrorReporter> kReporter = std::make_shared<DefaultErrorReporter>();
  return kReporter;
}

std::unique_ptr<ErrorReporter> ErrorReporter::DefaultUnique() {
  return std::make_unique<DefaultErrorReporter>();
}

}  // namespace scenic_impl
