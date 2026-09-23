// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <zircon/assert.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/logging.h"

namespace display::internal {

namespace {

fx_log_severity_t ToFxLogSeverity(DriverLogSeverity driver_log_severity) {
  switch (driver_log_severity) {
    case DriverLogSeverity::kTRACE:
      return DDK_LOG_TRACE;
    case DriverLogSeverity::kDEBUG:
      return DDK_LOG_DEBUG;
    case DriverLogSeverity::kINFO:
      return DDK_LOG_INFO;
    case DriverLogSeverity::kWARNING:
      return DDK_LOG_WARNING;
    case DriverLogSeverity::kERROR:
      return DDK_LOG_ERROR;
    case DriverLogSeverity::kFATAL:
      return static_cast<fx_log_severity_t>(0x60);
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid driver log severity: %d",
                      static_cast<int>(driver_log_severity));
}

}  // namespace

bool IsLogSeverityEnabled(DriverLogSeverity severity) {
  const zx_driver_t* driver = __zircon_driver_rec__.driver;
  const fx_log_severity_t fx_log_severity = ToFxLogSeverity(severity);
  return driver_log_severity_enabled_internal(driver, fx_log_severity);
}

void LogVariadicArgs(DriverLogSeverity severity, const char* file, int line, const char* format,
                     va_list args) {
  const zx_driver_t* driver = __zircon_driver_rec__.driver;
  const fx_log_severity_t fx_log_severity = ToFxLogSeverity(severity);
  if (driver_log_severity_enabled_internal(driver, fx_log_severity)) {
    driver_logvf_internal(driver, fx_log_severity, /*tag=*/nullptr, file, line, format, args);
  }
}

}  // namespace display::internal
