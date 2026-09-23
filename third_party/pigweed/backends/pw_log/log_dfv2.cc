// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>

#include "pw_log_fuchsia/log_fuchsia.h"

namespace {
inline FuchsiaLogSeverity LogLevelToLogSeverity(int level) {
  switch (level) {
    case PW_LOG_LEVEL_DEBUG:
      return FUCHSIA_LOG_DEBUG;
    case PW_LOG_LEVEL_INFO:
      return FUCHSIA_LOG_INFO;
    case PW_LOG_LEVEL_WARN:
      return FUCHSIA_LOG_WARNING;
    case PW_LOG_LEVEL_ERROR:
    case PW_LOG_LEVEL_CRITICAL:
      return FUCHSIA_LOG_ERROR;
    case PW_LOG_LEVEL_FATAL:
      return FUCHSIA_LOG_FATAL;
    default:
      return FUCHSIA_LOG_INFO;
  }
}
}  // namespace

extern "C" {
void pw_log_fuchsia_impl(int level, const char* module_name, const char* file_name, int line_number,
                         const char* message) {
  FuchsiaLogSeverity severity = LogLevelToLogSeverity(level);
  if (fdf::Logger::GlobalInstance()->GetSeverity() <= severity) {
    fdf::Logger::GlobalInstance()->logf(severity, module_name, file_name, line_number, "%s",
                                        message);
  }
}
}
