// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_STRUCTURED_BACKEND_CPP_RAW_LOG_SETTINGS_H_
#define LIB_SYSLOG_STRUCTURED_BACKEND_CPP_RAW_LOG_SETTINGS_H_

#include <lib/async/dispatcher.h>
#include <lib/syslog/structured_backend/fuchsia_syslog.h>
#include <zircon/availability.h>

namespace fuchsia_logging {

// This *must* be ABI stable.
struct RawLogSettings {
  FuchsiaLogSeverity min_log_level = FUCHSIA_LOG_INFO;
  zx_handle_t log_sink = ZX_HANDLE_INVALID;
  const char* const* tags = nullptr;
  size_t tags_count = 0;
  async_dispatcher_t* dispatcher = nullptr;
  void (*severity_change_callback)(void* context, FuchsiaLogSeverity severity) = nullptr;
  void* severity_change_callback_context = nullptr;
  uint64_t reserved[10] = {};
};

// Prevent surprises...
static_assert(sizeof(RawLogSettings) == 128);

}  // namespace fuchsia_logging

#endif  // LIB_SYSLOG_STRUCTURED_BACKEND_CPP_RAW_LOG_SETTINGS_H_
