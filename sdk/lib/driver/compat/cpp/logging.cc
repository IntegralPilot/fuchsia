// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/compat/cpp/logging.h>

#include <cstdarg>

bool driver_log_severity_enabled_internal(FuchsiaLogSeverity severity) {
  return fdf::Logger::GlobalInstance()->GetSeverity() <= severity;
}

void driver_logf_internal(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
                          const char* msg, ...) {
  va_list args;
  va_start(args, msg);
  driver_logvf_internal(severity, tag, file, line, msg, args);
  va_end(args);
}

void driver_logvf_internal(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
                           const char* msg, va_list args) {
  fdf::Logger::GlobalInstance()->logvf(severity, tag, file, line, msg, args);
}
