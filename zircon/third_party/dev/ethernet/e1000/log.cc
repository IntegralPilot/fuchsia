// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "log.h"

#include <stdarg.h>

#include <sdk/lib/driver/logging/cpp/logger.h>

__BEGIN_CDECLS

void e1000_logf(FuchsiaLogSeverity severity, const char* file, int line, const char* msg, ...) {
  va_list args;
  va_start(args, msg);
  if (fdf::Logger::GlobalInstance()) {
    fdf::Logger::GlobalInstance()->logvf(severity, nullptr, file, line, msg, args);
  }
  va_end(args);
}

__END_CDECLS
