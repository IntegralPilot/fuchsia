// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>
#include <stdarg.h>

#include <wlan/drivers/log_instance.h>

extern "C" void wlan_drivers_log_with_severity(FuchsiaLogSeverity severity, uint32_t filter,
                                               const char* tag, const char* file, int line,
                                               const char* fmt, ...) {
  using wlan::drivers::log::Instance;

  fdf::Logger* logger = fdf::Logger::GlobalInstance();

  va_list args;
  va_start(args, fmt);

  switch (severity) {
    case FUCHSIA_LOG_ERROR:
    case FUCHSIA_LOG_WARNING:
    case FUCHSIA_LOG_INFO:
      logger->logvf(severity, tag, file, line, fmt, args);
      break;
    case FUCHSIA_LOG_DEBUG:
    case FUCHSIA_LOG_TRACE:
      if (Instance::IsFilterOn(filter)) {
        logger->logvf(severity, tag, file, line, fmt, args);
      }
      break;
    case FUCHSIA_LOG_NONE:
      break;
    default:
      fdf::warn("Unrecognized log severity: {}. Logging message with WARNING level instead.",
                severity);
      logger->logvf(FUCHSIA_LOG_WARNING, tag, file, line, fmt, args);
      break;
  }
  va_end(args);
}
