// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_LOGGING_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_LOGGING_H_

#include <zircon/compiler.h>

#include <cstdarg>
#include <cstdint>

// This header provides a compatibility layer for both DFv1 and DFv2 logging.
//
// Drivers should not use the types and methods defined in this header and
// should always use the macros defined in `zxlogf.h` instead.

namespace display::internal {

// Log severity compatible with both the DFv1 (DDK) severity and the DFv2
// (Fuchsia structured syslog) severity.
enum class DriverLogSeverity : uint8_t {
  // Equivalent to `DDK_LOG_TRACE` and `FUCHSIA_LOG_TRACE`.
  kTRACE = 0x10,
  // Equivalent to `DDK_LOG_DEBUG` and `FUCHSIA_LOG_DEBUG`.
  kDEBUG = 0x20,
  // Equivalent to `DDK_LOG_INFO` and `FUCHSIA_LOG_INFO`.
  kINFO = 0x30,
  // Equivalent to `DDK_LOG_WARNING` and `FUCHSIA_LOG_WARNING`.
  kWARNING = 0x40,
  // Equivalent to `DDK_LOG_ERROR` and `FUCHSIA_LOG_ERROR`.
  kERROR = 0x50,
  // Equivalent to `DDK_LOG_FATAL` and `FUCHSIA_LOG_FATAL`.
  kFATAL = 0x60,
};

// Do not use this function directly. Use `zxlog_level_enabled()` defined in
// `zxlogf.h` instead.
bool IsLogSeverityEnabled(DriverLogSeverity severity);

// Do not use this function directly. Use `zxlogf()` defined in `zxlogf.h`
// instead.
void LogVariadicArgs(DriverLogSeverity severity, const char* file, int line, const char* format,
                     va_list args);

// Do not use this function directly. Use `zxlogf()` defined in `zxlogf.h`
// instead.
inline void __PRINTFLIKE(4, 5)
    Log(DriverLogSeverity severity, const char* file, int line, const char* format, ...) {
  va_list args;
  va_start(args, format);
  LogVariadicArgs(severity, file, line, format, args);
  va_end(args);
}

}  // namespace display::internal

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_LOGGING_H_
