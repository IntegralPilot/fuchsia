// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/log_level.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <stdlib.h>

#include <string>

#include <log/log.h>

namespace {
std::string format(const char* fmt, va_list args) {
  va_list args_copy;
  va_copy(args_copy, args);
  char buf[32];
  size_t n = std::vsnprintf(buf, sizeof(buf), fmt, args_copy);
  va_end(args_copy);

  // Check whether the static buffer was big enough.
  if (n < sizeof(buf)) {
    return {buf, n};
  }

  // Allocate a buffer of the correct size.
  std::string s(n + 1, 0);
  std::vsnprintf(const_cast<char*>(s.data()), s.size(), fmt, args);

  return s;
}

fuchsia_logging::LogSeverity GetSeverity(int prio) {
  switch (prio) {
    case ANDROID_LOG_VERBOSE:
      return fuchsia_logging::LogSeverity::Trace;
    case ANDROID_LOG_DEBUG:
      return fuchsia_logging::LogSeverity::Debug;
    case ANDROID_LOG_INFO:
      return fuchsia_logging::LogSeverity::Info;
    case ANDROID_LOG_WARN:
      return fuchsia_logging::LogSeverity::Warn;
    case ANDROID_LOG_ERROR:
      return fuchsia_logging::LogSeverity::Error;
    case ANDROID_LOG_FATAL:
      return fuchsia_logging::LogSeverity::Fatal;
    default:
      return fuchsia_logging::LogSeverity::Error;
  }
}
}  // namespace

int __android_log_error_write(int tag, const char* subTag, int32_t uid, const char* data,
                              uint32_t dataLen) {
  FX_LOG_KV(ERROR, std::string(data, dataLen).c_str(), FX_KV("tag", subTag), FX_KV("uid", uid));
  return 0;
}

void __android_log_write_log_message(__android_log_message* log_message) {
  fuchsia_logging::LogSeverity severity = GetSeverity(log_message->priority);
  if (::fuchsia_logging::IsSeverityEnabled(severity)) {
    if (log_message->tag) {
      fuchsia_logging::internal::WriteStructuredLog(severity, log_message->file, log_message->line,
                                                    log_message->message,
                                                    FX_KV("tag", log_message->tag));
    } else {
      fuchsia_logging::internal::WriteStructuredLog(severity, log_message->file, log_message->line,
                                                    log_message->message);
    }
  }
}

int __android_log_is_loggable(int prio, const char* tag, int default_prio) {
  return ::fuchsia_logging::IsSeverityEnabled(GetSeverity(prio));
}

void __android_log_call_aborter(const char* abort_message) { FX_LOGS(FATAL) << abort_message; }

void __android_log_assert(const char* cond, const char* tag, const char* fmt, ...) {
  std::string message;
  if (fmt) {
    va_list args;
    va_start(args, fmt);
    message = format(fmt, args);
    va_end(args);
  } else {
    if (cond) {
      message = "Assertion failed: " + std::string(cond);
    } else {
      message = "Unspecified assertion failed";
    }
  }
  __android_log_message log_message;
  log_message.priority = ANDROID_LOG_FATAL;
  log_message.file = nullptr;
  log_message.line = 0;
  log_message.tag = tag;
  log_message.message = message.c_str();
  __android_log_write_log_message(&log_message);
  FX_NOTREACHED();
  abort();
}

int __android_log_print(int prio, const char* tag, const char* fmt, ...) {
  va_list args;
  va_start(args, fmt);
  std::string message = format(fmt, args);
  va_end(args);

  __android_log_message log_message;
  log_message.priority = prio;
  log_message.file = nullptr;
  log_message.line = 0;
  log_message.tag = tag;
  log_message.message = message.c_str();
  __android_log_write_log_message(&log_message);
  return 0;
}

int __android_log_is_loggable_len(int prio, const char* tag, size_t len, int default_prio) {
  return __android_log_is_loggable(prio, tag, default_prio);
}

void __android_log_set_logger(__android_logger_function logger) {}
