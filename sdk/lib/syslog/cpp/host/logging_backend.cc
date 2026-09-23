// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <fcntl.h>
#include <inttypes.h>
#include <lib/syslog/cpp/log_level.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <unistd.h>

#include <iostream>
#include <sstream>

#include "lib/syslog/cpp/host/encoder.h"

namespace fuchsia_logging {

namespace {
// It's OK to keep global state here even though this file is in a source_set because on host
// we don't use shared libraries.
LogSettings g_log_settings;

std::string_view StripDots(std::string_view path) {
  auto pos = path.rfind("../");
  return pos == std::string_view::npos ? path : path.substr(pos + 3);
}

void BeginRecordLegacy(LogBuffer* buffer, RawLogSeverity severity,
                       std::optional<std::string_view> file, unsigned int line,
                       std::optional<std::string_view> msg,
                       std::optional<std::string_view> condition) {
  if (!file) {
    file = "";
  }
  auto header = internal::MsgHeader::CreatePtr(buffer);
  header->buffer = buffer;
  header->Init(buffer, severity);
#ifndef __Fuchsia__
  auto severity_string = internal::GetNameForLogSeverity(severity);
  header->WriteString(severity_string.data());
  header->WriteString(": ");
#endif
  header->WriteChar('[');
  header->WriteString(StripDots(*file));
  header->WriteChar('(');
  char a_buffer[128];
  snprintf(a_buffer, 128, "%i", line);
  header->WriteString(a_buffer);
  header->WriteString(")] ");
  if (condition) {
    header->WriteString("Check failed: ");
    header->WriteString(*condition);
    header->WriteString(". ");
  }
  if (msg) {
    header->WriteString(*msg);
    header->has_msg = true;
  }
}

void EndRecordLegacy(LogBuffer* buffer) {}

}  // namespace
namespace internal {
const std::string GetNameForLogSeverity(RawLogSeverity severity) {
  switch (severity) {
    case LogSeverity::Trace:
      return "TRACE";
    case LogSeverity::Debug:
      return "DEBUG";
    case LogSeverity::Info:
      return "INFO";
    case LogSeverity::Warn:
      return "WARNING";
    case LogSeverity::Error:
      return "ERROR";
    case LogSeverity::Fatal:
      return "FATAL";
  }

  if (severity > LogSeverity::Debug && severity < LogSeverity::Info) {
    std::ostringstream stream;
    stream << "VLOG(" << (LogSeverity::Info - severity) << ")";
    return stream.str();
  }

  return "UNKNOWN";
}
}  // namespace internal
namespace {
void SetLogSettings(const LogSettings& settings) {
  g_log_settings.min_log_level =
      std::min(static_cast<uint8_t>(LogSeverity::Fatal), settings.min_log_level);

  const char* raw_severity_from_env = std::getenv("FUCHSIA_HOST_LOG_MIN_SEVERITY");
  if (raw_severity_from_env) {
    std::string severity_from_env(raw_severity_from_env);
    if (severity_from_env == "FATAL") {
      g_log_settings.min_log_level =
          std::min(static_cast<RawLogSeverity>(LogSeverity::Fatal), g_log_settings.min_log_level);
    } else if (severity_from_env == "ERROR") {
      g_log_settings.min_log_level =
          std::min(static_cast<RawLogSeverity>(LogSeverity::Error), g_log_settings.min_log_level);
    } else if (severity_from_env == "INFO") {
      g_log_settings.min_log_level =
          std::min(static_cast<RawLogSeverity>(LogSeverity::Info), g_log_settings.min_log_level);
    } else if (severity_from_env == "DEBUG") {
      g_log_settings.min_log_level =
          std::min(static_cast<RawLogSeverity>(LogSeverity::Debug), g_log_settings.min_log_level);
    } else if (severity_from_env == "TRACE") {
      g_log_settings.min_log_level =
          std::min(static_cast<RawLogSeverity>(LogSeverity::Trace), g_log_settings.min_log_level);
    }
  }

  if (g_log_settings.log_file != settings.log_file) {
    if (!settings.log_file.empty()) {
      int fd = open(settings.log_file.c_str(), O_WRONLY | O_CREAT | O_APPEND, S_IRUSR | S_IWUSR);
      if (fd < 0) {
        std::cerr << "Could not open log file: " << settings.log_file << " (" << strerror(errno)
                  << ")" << std::endl;
      } else {
        // Redirect stderr to file.
        if (dup2(fd, STDERR_FILENO) < 0) {
          std::cerr << "Could not set stderr to log file: " << settings.log_file << " ("
                    << strerror(errno) << ")" << std::endl;
        } else {
          g_log_settings.log_file = settings.log_file;
        }
        close(fd);
      }
    }
  }
}
}  // namespace
void SetLogTags(const std::initializer_list<std::string>& tags) {
  // Global tags aren't supported on host.
}

RawLogSeverity GetMinLogSeverity() { return g_log_settings.min_log_level; }

void BeginRecord(LogBuffer* buffer, RawLogSeverity severity, internal::NullSafeStringView file,
                 unsigned int line, internal::NullSafeStringView msg,
                 internal::NullSafeStringView condition) {
  BeginRecordLegacy(buffer, severity, file, line, msg, condition);
}

void EndRecord(LogBuffer* buffer) { EndRecordLegacy(buffer); }

void WriteLog(LogSeverity severity, const char* file, unsigned int line, const char* tag,
              const char* condition, const std::string& msg) {
  if (tag)
    std::cerr << "[" << tag << "] ";

  std::cerr << "[" << internal::GetNameForLogSeverity(severity) << ":" << file << "(" << line
            << ")]";

  if (condition)
    std::cerr << " Check failed: " << condition << ".";

  std::cerr << msg << std::endl;
  std::cerr.flush();
}

// Sets the default log severity. If not explicitly set,
// this defaults to INFO, or to the value specified by Archivist.
LogSettingsBuilder& LogSettingsBuilder::WithMinLogSeverity(RawLogSeverity min_log_level) {
  settings_.min_log_level = min_log_level;
  return *this;
}

// Sets the log file.
LogSettingsBuilder& LogSettingsBuilder::WithLogFile(const std::string_view& log_file) {
  settings_.log_file = log_file;
  return *this;
}

LogSettingsBuilder& LogSettingsBuilder::WithTags(const std::initializer_list<std::string>& tags) {
  for (auto& tag : tags) {
    settings_.tags.push_back(tag);
  }
  return *this;
}

// Configures the log settings.
void LogSettingsBuilder::BuildAndInitialize() { SetLogSettings(settings_); }

LogBuffer LogBufferBuilder::Build() {
  LogBuffer buffer;
  BeginRecord(&buffer, severity_,
              fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(file_name_), line_,
              fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(msg_),
              fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(condition_));
  return buffer;
}

zx::result<> FlushToGlobalLogger(LogBuffer& buffer) {
  auto header = internal::MsgHeader::CreatePtr(&buffer);
  *(header->offset++) = 0;
  if (header->user_tag) {
    auto tag = header->user_tag;
    std::cerr << "[" << tag << "] ";
  }
  std::cerr << reinterpret_cast<const char*>(buffer.data()) << std::endl;
  return zx::ok();
}

}  // namespace fuchsia_logging
