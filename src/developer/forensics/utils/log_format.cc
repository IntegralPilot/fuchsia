// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/log_format.h"

#include <fuchsia/logger/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/log_level.h>
#include <lib/syslog/cpp/macros.h>

#include <cinttypes>

#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics {
namespace {

constexpr char kDroppedOnceFormatStr[] = "!!! DROPPED 1 LOG !!!";
constexpr char kDroppedFormatStr[] = "!!! DROPPED %u LOGS !!!";

std::string SeverityToString(const int32_t severity) {
  if (severity == FUCHSIA_LOG_TRACE) {
    return "TRACE";
  } else if (severity == FUCHSIA_LOG_DEBUG) {
    return "DEBUG";
  } else if (severity > FUCHSIA_LOG_DEBUG && severity < FUCHSIA_LOG_INFO) {
    return fxl::StringPrintf("VLOG(%d)", FUCHSIA_LOG_INFO - severity);
  } else if (severity == FUCHSIA_LOG_INFO) {
    return "INFO";
  } else if (severity == FUCHSIA_LOG_WARNING) {
    return "WARN";
  } else if (severity == FUCHSIA_LOG_ERROR) {
    return "ERROR";
  } else if (severity == FUCHSIA_LOG_FATAL) {
    return "FATAL";
  }
  return "INVALID";
}

fit::function<std::string(int32_t, const std::string&)> FormatFn(
    const fuchsia::logger::LogMessage& message) {
  return [&message](int32_t severity, const std::string& msg) {
    return fxl::StringPrintf("[%05d.%03d][%05" PRIu64 "][%05" PRIu64 "][%s] %s: %s\n",
                             static_cast<int>(message.time.get() / 1000000000ULL),
                             static_cast<int>((message.time.get() / 1000000ULL) % 1000ULL),
                             message.pid, message.tid, fxl::JoinStrings(message.tags, ", ").c_str(),
                             SeverityToString(severity).c_str(), msg.c_str());
  };
}

}  // namespace

std::string Format(const fuchsia::logger::LogMessage& message) {
  fit::function<std::string(int32_t, const std::string&)> format = FormatFn(message);

  std::string log;
  if (message.dropped_logs == 1) {
    log += format(FUCHSIA_LOG_WARNING, kDroppedOnceFormatStr);
  } else if (message.dropped_logs > 1) {
    log += format(FUCHSIA_LOG_WARNING, fxl::StringPrintf(kDroppedFormatStr, message.dropped_logs));
  }

  log += format(message.severity, message.msg);

  return log;
}

}  // namespace forensics
