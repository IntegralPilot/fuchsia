// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/shared/logging/logging.h"

#include <lib/syslog/cpp/log_level.h>
#include <lib/syslog/cpp/macros.h>
#include <stdio.h>

#include <iostream>

namespace debug {

namespace {

LogBackend* log_backend = nullptr;
bool log_enable_syslog = true;

fuchsia_logging::LogSeverity ConvertSeverity(LogSeverity severity) {
  switch (severity) {
    case LogSeverity::kInfo:
      return fuchsia_logging::LogSeverity::Info;
    case LogSeverity::kWarn:
      return fuchsia_logging::LogSeverity::Warn;
    case LogSeverity::kError:
      return fuchsia_logging::LogSeverity::Error;
  }
}

}  // namespace

LogStatement::~LogStatement() {
  if (log_backend) {
    log_backend->WriteLog(severity_, location_, stream_.str());
  }
  if (log_enable_syslog) {
    auto severity = ConvertSeverity(severity_);
    if (fuchsia_logging::IsSeverityEnabled(severity)) {
      fuchsia_logging::LogMessage(severity, location_.file(), static_cast<int>(location_.line()),
                                  nullptr, nullptr)
              .stream()
          << stream_.str();
    }
  }
}

void LogBackend::Set(LogBackend* backend, bool enable_syslog) {
  log_backend = backend;
  log_enable_syslog = enable_syslog;
}

void LogBackend::Unset() { Set(nullptr, true); }

LogBackend::~LogBackend() = default;

}  // namespace debug
