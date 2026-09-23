// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <cstring>
#include <iostream>
#include <memory>

#ifdef __Fuchsia__
#include <zircon/status.h>
#endif

namespace fuchsia_logging {
namespace {

const char* StripDots(const char* path) {
  while (strncmp(path, "../", 3) == 0)
    path += 3;
  return path;
}

const char* StripPath(const char* path) {
  auto p = strrchr(path, '/');
  if (p)
    return p + 1;
  else
    return path;
}

}  // namespace

LogMessage::LogMessage(RawLogSeverity severity, const char* file, int line, const char* condition,
                       const char* tag
#if defined(__Fuchsia__)
                       ,
                       zx_status_t status
#endif
                       )
    : severity_(severity),
      file_(file ? (severity_ > fuchsia_logging::LogSeverity::Info ? StripDots(file)
                                                                   : StripPath(file))
                 : nullptr),
      line_(line),
      condition_(condition),
      tag_(tag)
#if defined(__Fuchsia__)
      ,
      status_(status)
#endif
{
}

LogMessage::~LogMessage() {
#if defined(__Fuchsia__)
  if (status_ != std::numeric_limits<zx_status_t>::max()) {
    stream_ << ": " << status_ << " (" << zx_status_get_string(status_) << ")";
  }
#endif
  auto str = stream_.str();
  LogBufferBuilder builder(severity_);
  if (condition_) {
    builder.WithCondition(condition_);
  }
  builder.WithMsg(str);
  if (file_) {
    builder.WithFile(file_, line_);
  }
  auto buffer = builder.Build();
  if (tag_) {
    buffer.WriteKeyValue("tag", tag_);
  }
#if defined(__Fuchsia__) && FUCHSIA_API_LEVEL_LESS_THAN(29)
  buffer.Flush();
#else
  [[maybe_unused]] zx::result<> result = FlushToGlobalLogger(buffer);
#endif
  if (severity_ >= fuchsia_logging::LogSeverity::Fatal) {
    if (condition_) {
      std::cerr << "Check failed: " << condition_ << ". ";
    }
    std::cerr << str << '\n';
    __builtin_debugtrap();
  }
}

bool LogFirstNState::ShouldLog(uint32_t n) {
  const uint32_t counter_value = counter_.fetch_add(1, std::memory_order_relaxed);
  return counter_value < n;
}

}  // namespace fuchsia_logging
