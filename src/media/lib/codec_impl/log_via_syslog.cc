// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/lib/codec_impl/include/lib/media/codec_impl/log.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdarg>

#include "src/lib/fxl/strings/string_printf.h"

namespace codec_impl {
namespace internal {

void log_via_environment(FuchsiaLogSeverity severity, const char* file, int line, const char* msg,
                         ...) {
  va_list args;
  va_start(args, msg);
  if (fuchsia_logging::IsSeverityEnabled(severity)) {
    std::string part2 = fxl::StringVPrintf(msg, args);
    fuchsia_logging::LogMessage(severity, BaseName(file), line, nullptr, "codec_impl").stream()
        << part2;
  }
  va_end(args);
}

}  // namespace internal
}  // namespace codec_impl
