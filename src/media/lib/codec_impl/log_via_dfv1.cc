// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>

#include <cstdarg>

#include "src/media/lib/codec_impl/include/lib/media/codec_impl/log.h"

namespace codec_impl {
namespace internal {

void log_via_environment(FuchsiaLogSeverity severity, const char* file, int line, const char* msg,
                         ...) {
  fx_log_severity_t flag = static_cast<fx_log_severity_t>(severity);

  va_list args;
  va_start(args, msg);
  if (driver_log_severity_enabled_internal(__zircon_driver_rec__.driver, flag)) {
    driver_logvf_internal(__zircon_driver_rec__.driver, flag, nullptr, BaseName(file), line, msg,
                          args);
  }
  va_end(args);
}

}  // namespace internal
}  // namespace codec_impl
