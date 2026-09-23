// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdarg>
#include <cstdio>
#include <string>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/media/lib/codec_impl/include/lib/media/codec_impl/log.h"

namespace codec_impl {
namespace internal {

void log_via_environment(FuchsiaLogSeverity severity, const char* file, int line, const char* msg,
                         ...) {
  va_list args;
  va_start(args, msg);
  std::string formatted_msg = fxl::StringVPrintf(msg, args);
  va_end(args);

  fprintf(stderr, "[codec_impl %s:%d] %s\n", BaseName(file), line, formatted_msg.c_str());
}

}  // namespace internal
}  // namespace codec_impl
