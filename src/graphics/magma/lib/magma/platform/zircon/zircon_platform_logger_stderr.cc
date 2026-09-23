// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_logger.h>
#include <stdio.h>

#include <iostream>

namespace magma {

void PlatformLogger::LogVa(LogLevel level, const char* file, int line, const char* fmt,
                           va_list args) {
  const char* level_string = nullptr;
  switch (level) {
    case PlatformLogger::LOG_ERROR:
      level_string = "ERROR";
      break;
    case PlatformLogger::LOG_WARNING:
      level_string = "WARNING";
      break;
    case PlatformLogger::LOG_INFO:
      level_string = "INFO";
      break;
  }

  fprintf(stderr, "%s: %s:%d  ", level_string, file, line);
  vfprintf(stderr, fmt, args);
  fprintf(stderr, "\n");
}

}  // namespace magma
