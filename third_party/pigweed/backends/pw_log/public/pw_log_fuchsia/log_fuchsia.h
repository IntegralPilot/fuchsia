// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef THIRD_PARTY_PIGWEED_BACKENDS_PW_LOG_PUBLIC_PW_LOG_FUCHSIA_LOG_FUCHSIA_H_
#define THIRD_PARTY_PIGWEED_BACKENDS_PW_LOG_PUBLIC_PW_LOG_FUCHSIA_LOG_FUCHSIA_H_
#include "pw_log/levels.h"

extern "C" {
void pw_log_fuchsia_impl(int level, const char* module_name, const char* file_name, int line_number,
                         const char* message);
}

namespace pw_log_fuchsia {
inline const char* LogLevelToString(int severity) {
  switch (severity) {
    case PW_LOG_LEVEL_ERROR:
      return "ERROR";
    case PW_LOG_LEVEL_WARN:
      return "WARN";
    case PW_LOG_LEVEL_INFO:
      return "INFO";
    case PW_LOG_LEVEL_DEBUG:
      return "DEBUG";
    default:
      return "UNKNOWN";
  }
}

// Returns the part of a path following the final '/', or the whole path if there is no '/'.
constexpr const char* BaseName(const char* path) {
  for (const char* c = path; c && (*c != '\0'); c++) {
    if (*c == '/') {
      path = c + 1;
    }
  }
  return path;
}

}  // namespace pw_log_fuchsia

#endif  // THIRD_PARTY_PIGWEED_BACKENDS_PW_LOG_PUBLIC_PW_LOG_FUCHSIA_LOG_FUCHSIA_H_
