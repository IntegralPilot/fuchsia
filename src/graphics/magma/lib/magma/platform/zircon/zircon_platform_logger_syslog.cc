// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <fidl/fuchsia.logger/cpp/wire.h>
#include <lib/magma/platform/platform_logger.h>
#include <lib/magma/platform/platform_logger_provider.h>
#include <lib/magma/platform/platform_thread.h>
#include <lib/syslog/structured_backend/cpp/log_buffer.h>
#include <lib/syslog/structured_backend/cpp/logger.h>
#include <lib/zx/channel.h>
#include <lib/zx/socket.h>
#include <stdarg.h>
#include <stdio.h>

#include <fbl/no_destructor.h>

#include "zircon_platform_handle.h"

namespace magma {
namespace {

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
bool g_is_logging_initialized = false;
// Intentionally leaked on shutdown to ensure there are no destructor ordering problems.
zx_handle_t log_socket;
#else
// This is a static function so there are no ordering issues on initialization of `global_logger`.
fuchsia_logging::Logger& GetGlobalLogger() {
  static fbl::NoDestructor<fuchsia_logging::Logger> global_logger;
  return *global_logger;
}
#endif

}  // namespace

bool PlatformLoggerProvider::IsInitialized() {
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  return g_is_logging_initialized;
#else
  return GetGlobalLogger().IsValid();
#endif
}

bool PlatformLoggerProvider::Initialize(std::unique_ptr<PlatformHandle> channel) {
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  zx::socket local_socket, remote_socket;
  zx_status_t status = zx::socket::create(ZX_SOCKET_DATAGRAM, &local_socket, &remote_socket);
  if (status != ZX_OK)
    return false;

  auto zircon_handle = static_cast<ZirconPlatformHandle*>(channel.get());

  auto result = fidl::WireCall(fidl::UnownedClientEnd<fuchsia_logger::LogSink>(
                                   zx::unowned_channel(zircon_handle->get())))
                    ->ConnectStructured(std::move(remote_socket));
  if (result.status() != ZX_OK)
    return false;

  log_socket = local_socket.release();

  g_is_logging_initialized = true;
#else
  auto zircon_handle = static_cast<ZirconPlatformHandle*>(channel.get());
  constexpr const char* kTags[] = {"magma"};
  if (auto logger = fuchsia_logging::Logger::Create(fuchsia_logging::RawLogSettings{
          .log_sink = zircon_handle->release(),
          .tags = kTags,
          .tags_count = std::size(kTags),
      });
      logger.is_error()) {
    return false;
  } else {
    GetGlobalLogger() = *std::move(logger);
  }
#endif
  return true;
}

static FuchsiaLogSeverity get_severity(PlatformLogger::LogLevel level) {
  switch (level) {
    case PlatformLogger::LOG_INFO:
      return FUCHSIA_LOG_INFO;
    case PlatformLogger::LOG_WARNING:
      return FUCHSIA_LOG_WARNING;
    case PlatformLogger::LOG_ERROR:
      return FUCHSIA_LOG_ERROR;
  }
}

void PlatformLogger::LogVa(LogLevel level, const char* file, int line, const char* msg,
                           va_list args) {
  if (!PlatformLoggerProvider::IsInitialized()) {
    return;
  }
  constexpr size_t kFormatStringLength = 1024;
  char fmt_string[kFormatStringLength];
  fmt_string[kFormatStringLength - 1] = 0;
  int n = kFormatStringLength;
  // Format
  // Number of bytes written not including null terminator
  int count = vsnprintf(fmt_string, n, msg, args);
  if (count < 0) {
    return;
  }

  // Add null terminator.
  count++;

  if (count >= n) {
    // truncated
    constexpr char kEllipsis[] = "...";
    constexpr size_t kEllipsisSize = sizeof(kEllipsis);
    snprintf(fmt_string + kFormatStringLength - 1 - kEllipsisSize, kEllipsisSize, kEllipsis);
  }

  std::string_view file_str(file);
  if (size_t last_slash = file_str.rfind('/'); last_slash != std::string_view::npos) {
    file_str.remove_prefix(last_slash + 1);
  }
  fuchsia_logging::LogBuffer log_buffer;
  uint64_t tid = PlatformThreadId().id();
  uint64_t pid = PlatformProcessHelper::GetCurrentProcessId();
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  log_buffer.BeginRecord(get_severity(level), file_str, line, fmt_string,
                         zx::unowned_socket(log_socket), 0, pid, tid);
  log_buffer.WriteKeyValue("tag", "magma");
  log_buffer.FlushRecord();
#else
  log_buffer.BeginRecord(get_severity(level), file_str, line, fmt_string, 0, pid, tid);
  [[maybe_unused]] auto result = GetGlobalLogger().FlushBuffer(log_buffer);
#endif
}

}  // namespace magma
