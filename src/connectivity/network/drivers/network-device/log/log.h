// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_LOG_LOG_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_LOG_LOG_H_

#include <zircon/compiler.h>

#include <cstdarg>

#include <sdk/lib/syslog/cpp/log_message_impl.h>

namespace network {
namespace internal {

void LogfImpl(fuchsia_logging::LogSeverity severity, const char* tag, const char* file, int line,
              const char* format, va_list args);

}  // namespace internal

void Logf(fuchsia_logging::LogSeverity severity, const char* tag, const char* file, int line,
          const char* format, ...) __PRINTFLIKE(5, 6);

inline void Logf(fuchsia_logging::LogSeverity severity, const char* tag, const char* file, int line,
                 const char* format, ...) {
  if (fuchsia_logging::IsSeverityEnabled(severity)) {
    va_list args;
    va_start(args, format);
    internal::LogfImpl(severity, tag, file, line, format, args);
    va_end(args);
  }
}

}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_LOG_LOG_H_
