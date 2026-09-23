// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPAT_CPP_LOGGING_H_
#define LIB_DRIVER_COMPAT_CPP_LOGGING_H_

#include <lib/driver/logging/cpp/logger.h>

// This library provides an compatibility layer for <lib/ddk/debug.h>.
// Notable differences:
// * severities use FUCHSIA_LOG prefix instead of DDK_LOG
// * _etc functions were removed in favor of calling _internal functions directly.
// * The logger must be initialized by drivers by invoking driver_initialize_logger() before
// logging.
//
// A similar limitation to original DFv1 APIs: Logs will be misattributed if the same driver is
// loaded into the driver host multiple times.
__BEGIN_CDECLS

// Do not use this function directly, use zxlog_level_enabled() instead.
bool driver_log_severity_enabled_internal(FuchsiaLogSeverity flag);

// zxlog_level_enabled() provides a way for a driver to test to see if a
// particular log level is currently enabled.  This allows for patterns where a
// driver might want to log something at debug or trace level, but the something
// that they want to log might involve a computation or for loop which cannot be
// embedded into the log macro and therefor disabled without cost.
//
// Example:
// if (zxlog_level_enabled(DEBUG)) {
//     zxlogf(DEBUG, "Scatter gather table has %u entries", sg_table.count);
//     for (uint32_t i = 0; i < sg_table.count; ++i) {
//         zxlogf(DEBUG, "[%u] : 0x%08x, %u",
//                i, sg_table.entry[i].base, sg_table.entry[i].base);
//     }
// }
#define zxlog_level_enabled(flag) driver_log_severity_enabled_internal(FUCHSIA_LOG_##flag)

// Do not use this function directly, use zxlogf() instead.
void driver_logf_internal(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
                          const char* msg, ...) __PRINTFLIKE(5, 6);

// Do not use this function directly, use zxlogvf() instead.
void driver_logvf_internal(FuchsiaLogSeverity flag, const char* tag, const char* file, int line,
                           const char* msg, va_list args);

// zxlogf() provides a path to the kernel debuglog gated by log level flags
//
// Example:  zxlogf(ERROR, "oh no! ...");
//
// By default drivers have ERROR, WARN, and INFO debug levels enabled.
// The kernel commandline option driver.NAME.log may be used to override
// this. NAME is specified via the ZIRCON_DRIVER macro that declares the driver.
// The levels are the strings "error", "warning", "info", "debug", or "trace".
//
// Example driver.floppydisk.log=trace
//
#define zxlogf(flag, msg...) driver_logf_internal(FUCHSIA_LOG_##flag, NULL, __FILE__, __LINE__, msg)
#define zxlogf_tag(flag, tag, msg...) \
  driver_logf_internal(FUCHSIA_LOG_##flag, tag, __FILE__, __LINE__, msg)

// zxlogvf() is similar to zxlogf() but it accepts a va_list as argument,
// analogous to vprintf() and printf().
//
// Examples:
//   void log(const char* file, const char* line, const char* format, ...) {
//     std::string new_format = std::string("[tag] ") + format;
//
//     va_list args;
//     va_start(args, format);
//     zxlogvf(file, line, new_format.c_str(), args);
//     va_end(args);
//   }
//
// The debug levels are the same as those of zxlogf() macro.
//
#define zxlogvf(flag, file, line, format, args) \
  driver_logvf_internal(FUCHSIA_LOG_##flag, NULL, file, line, format, args);
#define zxlogvf_tag(flag, tag, file, line, format, args) \
  driver_logvf_internal(FUCHSIA_LOG_##flag, tag, file, line, format, args);

__END_CDECLS

#endif  // LIB_DRIVER_COMPAT_CPP_LOGGING_H_
