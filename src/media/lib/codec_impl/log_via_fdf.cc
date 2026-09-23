// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdarg>

#include "src/media/lib/codec_impl/include/lib/media/codec_impl/log.h"

namespace codec_impl {
namespace internal {

void log_via_environment(FuchsiaLogSeverity severity, const char* file, int line, const char* msg,
                         ...) {
  va_list args;
  va_start(args, msg);
  fdf::Logger::GlobalInstance()->logvf(severity, nullptr, BaseName(file), line, msg, args);
  va_end(args);
}

}  // namespace internal
}  // namespace codec_impl
