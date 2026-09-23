// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This is a header file to allow logging on Fuchsia.
// It is separate from the main magma.h in order to use vararg
// functions, which the generator doesn't currently support.

#ifndef LIB_MAGMA_MAGMA_LOGGING_H_
#define LIB_MAGMA_MAGMA_LOGGING_H_

#include <lib/magma/magma_common_defs.h>  // IWYU pragma: export
#include <stdarg.h>
#include <stdint.h>

// LINT.IfChange
#if defined(__cplusplus)
extern "C" {
#endif

///
/// \brief Logs a message to the Fuchsia logging service.  Use this instead of Fuchsia logging
///        directly because the latter involves a shared library dependency without ABI stability
///        guarantees.  NOTE: magma_initialize_logging must be called first.
/// \param severity The severity of the log message.
/// \param tag The tag of the log message
/// \param file The file name generating the log
/// \param line The line number generating the log
/// \param format The format string for the log message
/// \param va Arguments used in the format string
///
MAGMA_EXPORT void magma_log(magma_log_severity_t severity, const char* tag, const char* file,
                            int line, const char* format, va_list va) MAGMA_AVAILABLE_SINCE(30);

///
/// \brief Logs a message using Fuchsia structured logs
/// \param severity The Fuchsia severity of the log message.
/// \param tag The tag of the log message
/// \param file The file name generating the log
/// \param line The line number generating the log
/// \param format The format string for the log message
/// \param va Arguments used in the format string
///
MAGMA_EXPORT void magma_fuchsia_log(int8_t severity, const char* tag, const char* file, int line,
                                    const char* format, va_list va)
    MAGMA_DEPRECATED_SINCE(1, 30, "use magma_log");

#if defined(__cplusplus)
}
#endif

// LINT.ThenChange(/sdk/lib/magma_common/include/lib/magma/magma_common_defs.h:version)
#endif  // LIB_MAGMA_MAGMA_LOGGING_H_
