// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pw_log_sink/log_sink.h"

#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/structured_backend/cpp/logger.h>
#include <zircon/process.h>

#include "pw_log_fuchsia/log_fuchsia.h"

namespace {

FuchsiaLogSeverity PigweedLevelToFuchsiaSeverity(int pw_level) {
  switch (pw_level) {
    case PW_LOG_LEVEL_ERROR:
      return FUCHSIA_LOG_ERROR;
    case PW_LOG_LEVEL_WARN:
      return FUCHSIA_LOG_WARNING;
    case PW_LOG_LEVEL_INFO:
      return FUCHSIA_LOG_INFO;
    case PW_LOG_LEVEL_DEBUG:
      return FUCHSIA_LOG_DEBUG;
    default:
      return FUCHSIA_LOG_ERROR;
  }
}

constinit std::optional<fuchsia_logging::Logger> global_logger;

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

thread_local const zx_koid_t thread_koid = GetKoid(zx_thread_self());
zx_koid_t const process_koid = GetKoid(zx_process_self());

}  // namespace

namespace pw_log_sink {

void InitializeLogging(async_dispatcher_t* dispatcher) {
  auto client_end = component::Connect<fuchsia_logger::LogSink>();
  if (client_end.is_error()) {
    return;
  }
  auto logger = fuchsia_logging::Logger::Create(fuchsia_logging::RawLogSettings{
      .log_sink = client_end->TakeChannel().release(),
      .dispatcher = dispatcher,
  });
  if (logger.is_error()) {
    return;
  }
  global_logger = *std::move(logger);
}

}  // namespace pw_log_sink

extern "C" {

void pw_log_fuchsia_impl(int level, const char* module_name, const char* file_name, int line_number,
                         const char* message) {
  if (!global_logger) {
    return;
  }
  FuchsiaLogSeverity fuchsia_severity = PigweedLevelToFuchsiaSeverity(level);
  if (global_logger->GetMinSeverity() > fuchsia_severity) {
    return;
  }
  fuchsia_logging::LogBuffer buffer;
  buffer.BeginRecord(fuchsia_severity, std::string_view(file_name), line_number,
                     std::string_view(message), /*dropped_count=*/0, process_koid, thread_koid);
  buffer.WriteKeyValue("tag", module_name);
  [[maybe_unused]] zx::result<> result = global_logger->FlushBuffer(buffer);
}

}  // extern C
