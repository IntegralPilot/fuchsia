// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_CPP_MACROS_H_
#define LIB_SYSLOG_CPP_MACROS_H_

#include <lib/syslog/cpp/log_message_impl.h>

#define __FX_LOG_SEVERITY_TRACE (fuchsia_logging::LogSeverity::Trace)
#define __FX_LOG_SEVERITY_DEBUG (fuchsia_logging::LogSeverity::Debug)
#define __FX_LOG_SEVERITY_INFO (fuchsia_logging::LogSeverity::Info)
#define __FX_LOG_SEVERITY_WARNING (fuchsia_logging::LogSeverity::Warn)
#define __FX_LOG_SEVERITY_ERROR (fuchsia_logging::LogSeverity::Error)
#define __FX_LOG_SEVERITY_FATAL (fuchsia_logging::LogSeverity::Fatal)

/// Used for stream-based logging with a custom tag.
#define FX_LOG_STREAM(severity, tag)                                                            \
  ::fuchsia_logging::LogMessage(__FX_LOG_SEVERITY_##severity, __FILE__, __LINE__, nullptr, tag) \
      .stream()

/// Used for stream-based logging of a zx_status_t combined
/// with a log message.
#define FX_LOG_STREAM_STATUS(severity, status, tag)                                             \
  ::fuchsia_logging::LogMessage(__FX_LOG_SEVERITY_##severity, __FILE__, __LINE__, nullptr, tag, \
                                status)                                                         \
      .stream()

// Internal macro used by other macros
#define FX_LAZY_STREAM(stream, condition) \
  !(condition) ? (void)0 : ::fuchsia_logging::LogMessageVoidify() & (stream)

// Internal macro used by other macros
#define FX_EAT_STREAM_PARAMETERS(ignored)                                                  \
  true || (ignored)                                                                        \
      ? (void)0                                                                            \
      : ::fuchsia_logging::LogMessageVoidify() &                                           \
            ::fuchsia_logging::LogMessage(__FX_LOG_SEVERITY_FATAL, 0, 0, nullptr, nullptr) \
                .stream()

// Checks if a given severity level is enabled.
// Intended for use by other macros in this file.
#define FX_LOG_IS_ON(severity) (::fuchsia_logging::IsSeverityEnabled(__FX_LOG_SEVERITY_##severity))

/// Logs a message with a given severity level
#define FX_LOGS(severity) FX_LOGST(severity, nullptr)

/// Logs a message with a given severity level and tag.
#define FX_LOGST(severity, tag) FX_LAZY_STREAM(FX_LOG_STREAM(severity, tag), FX_LOG_IS_ON(severity))

#if defined(__Fuchsia__)
#define FX_PLOGST(severity, tag, status) \
  FX_LAZY_STREAM(FX_LOG_STREAM_STATUS(severity, status, tag), FX_LOG_IS_ON(severity))
#define FX_PLOGS(severity, status) FX_PLOGST(severity, nullptr, status)
#endif

/// Writes a message to the global logger, the first |n| times that any callsite
/// of this macro is invoked. |n| should be a positive integer literal.
/// |severity| is one of INFO, WARNING, ERROR, FATAL
///
/// Implementation notes:
/// The outer for loop is a trick to allow us to introduce a new scope and
/// introduce the variable |do_log| into that scope. It executes exactly once.
///
/// The inner for loop is a trick to allow us to introduce a new scope and
/// introduce the static variable |internal_state| into that new scope. It
/// executes either zero or one times.
///
/// C++ does not allow us to introduce two new variables into a single for loop
/// scope and we need |do_log| so that the inner for loop doesn't execute twice.
#define FX_FIRST_N(n, log_statement)                              \
  for (bool do_log = true; do_log; do_log = false)                \
    for (static ::fuchsia_logging::LogFirstNState internal_state; \
         do_log && internal_state.ShouldLog(n); do_log = false)   \
  log_statement
#define FX_LOGS_FIRST_N(severity, n) FX_FIRST_N(n, FX_LOGS(severity))
#define FX_LOGST_FIRST_N(severity, n, tag) FX_FIRST_N(n, FX_LOGST(severity, tag))

#define FX_CHECK(condition) FX_CHECKT(condition, nullptr)

#define FX_CHECKT(condition, tag)                                                                 \
  FX_LAZY_STREAM(                                                                                 \
      ::fuchsia_logging::LogMessage(__FX_LOG_SEVERITY_FATAL, __FILE__, __LINE__, #condition, tag) \
          .stream(),                                                                              \
      !(condition))

// Macros used to log based on whether or not NDEBUG is defined
#ifndef NDEBUG
#define FX_DLOGS(severity) FX_LOGS(severity)
#define FX_DCHECK(condition) FX_CHECK(condition)
#else
#define FX_DLOGS(severity) FX_EAT_STREAM_PARAMETERS(true)
#define FX_DCHECK(condition) FX_EAT_STREAM_PARAMETERS(condition)
#endif

/// Used to indicate unreachable code.
/// Crashes if this is hit.
#define FX_NOTREACHED() FX_DCHECK(false)

/// Used to indicate unimplemented code.
/// This doesn't crash, but prints an error.
#define FX_NOTIMPLEMENTED() FX_LOGS(ERROR) << "Not implemented in: " << __PRETTY_FUNCTION__

// Used internally by FX_LOG_KV
#define FX_LOG_KV_ETC(severity, args...)                                                   \
  do {                                                                                     \
    if (::fuchsia_logging::IsSeverityEnabled(severity)) {                                  \
      ::fuchsia_logging::internal::WriteStructuredLog(severity, __FILE__, __LINE__, args); \
    }                                                                                      \
  } while (0)

/// Used to denote a key-value pair for use in structured logging API calls.
/// This macro exists solely to improve readability of calls to FX_LOG_KV
#define FX_KV(a, b) ::fuchsia_logging::KeyValue(a, b)

/// Logs a structured message with a given severity,
/// message, and optional key-value pairs.
/// Example usage:
/// FX_LOG_KV(INFO, "Test message", FX_KV("meaning_of_life", 42));
#define FX_LOG_KV(severity, msg...) FX_LOG_KV_ETC(__FX_LOG_SEVERITY_##severity, msg)

#endif  // LIB_SYSLOG_CPP_MACROS_H_
