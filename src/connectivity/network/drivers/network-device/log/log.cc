// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/drivers/network-device/log/log.h"

namespace network::internal {

void LogfImpl(fuchsia_logging::LogSeverity severity, const char* tag, const char* file, int line,
              const char* format, va_list args) {
  char buffer[1024];
  vsnprintf(buffer, sizeof(buffer), format, args);
  fuchsia_logging::LogMessage(severity, file, line, nullptr, tag).stream() << buffer;
}

}  // namespace network::internal
