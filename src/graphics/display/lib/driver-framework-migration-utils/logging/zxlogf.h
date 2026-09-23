// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_ZXLOGF_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_ZXLOGF_H_

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/logging.h"

// This header provides driver logging macros compatible with both Driver
// Framework v1 (DFv1, <lib/ddk/debug.h>) and Driver Framework v2 (DFv2,
// <lib/driver/compat/cpp/logging.h>).
//
// Driver libraries using this header can compile into libraries linkable with
// both DFv1 (DDK) and DFv2 (driver runtime) libraries.
//
// The compatibility is achieved by redefining zxlogf() and
// zxlog_level_enabled() macros, so this header must not be used with DFv1 or
// DFv2 logging headers at the same time.
//
// To use this header in a driver library, replace all the existing DFv1 / DFv2
// logging headers with `zxlogf.h` and add `//src/graphics/display/lib/
// driver-framework-migration-utils/logging:zxlogf` to the library's deps.
//
// Note: DFv2 drivers and tests must register a logger by invoking
// `fdf::Logger::SetGlobalInstance()` before logging. DFv2 drivers implementing
// `fdf::DriverBase` always register the logger.

#ifdef zxlogf
#error \
    "zxlogf() already defined. This header must not be included "\
       "with <lib/ddk/debug.h> or <lib/driver/compat/logging.h>."
#endif  // zxlogf

#ifdef zxlog_level_enabled
#error \
    "zxlog_level_enabled() already defined. This header must not be included "\
       "with <lib/ddk/debug.h> or <lib/driver/compat/logging.h>."
#endif  // zxlog_level_enabled

// zxlog_level_enabled() returns true iff a particular log `severity` level is
// currently enabled.
//
// `severity` must be one of TRACE, DEBUG, INFO, WARNING and ERROR.
//
// Usage is the same as the zxlog_level_enabled() macro defined in
// <lib/ddk/debug.h> and <lib/driver/compat/cpp/logging.h>.
#define zxlog_level_enabled(severity) \
  ::display::internal::IsLogSeverityEnabled(::display::internal::DriverLogSeverity::k##severity)

// zxlogf() logs formatted contents to the driver's logger.
//
// `severity` must be one of TRACE, DEBUG, INFO, WARNING and ERROR.
//
// Usage is the same as the zxlogf() macro defined in <lib/ddk/debug.h> and
// <lib/driver/compat/cpp/logging.h>.
#define zxlogf(severity, format...)                                                       \
  ::display::internal::Log(::display::internal::DriverLogSeverity::k##severity, __FILE__, \
                           __LINE__, format)

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_ZXLOGF_H_
