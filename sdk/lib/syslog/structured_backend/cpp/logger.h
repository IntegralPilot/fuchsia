// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_STRUCTURED_BACKEND_CPP_LOGGER_H_
#define LIB_SYSLOG_STRUCTURED_BACKEND_CPP_LOGGER_H_

#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(29)

#include <lib/fit/function.h>
#include <lib/syslog/structured_backend/cpp/log_buffer.h>
#include <lib/syslog/structured_backend/cpp/raw_log_settings.h>
#include <lib/syslog/structured_backend/fuchsia_syslog.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <atomic>
#include <memory>
#include <string>

namespace fuchsia_logging {
namespace internal {
// Defined in logging_backend_fuchsia_globals.cc.
class Logger;
}  // namespace internal

class Logger {
 public:
  // Creates a new logger with the provided settings. If `min_severity` is non-null, it can point to
  // a a severity that will track the severity for the logger. The caller must guarantee it lives at
  // least as long as Logger does. If null, the minimum severity is stored internally.
  static zx::result<Logger> Create(const RawLogSettings& settings,
                                   std::atomic<FuchsiaLogSeverity>* min_severity = nullptr);

  Logger() = default;

  Logger(const Logger&) = default;
  Logger& operator=(const Logger&) = default;
  Logger(Logger&&) = default;
  Logger& operator=(Logger&&) = default;

  bool IsValid() const { return static_cast<bool>(impl_); }

  FuchsiaLogSeverity GetMinSeverity() const {
    // If no logger is configured, return FUCHSIA_LOG_FATAL rather than FUCHSIA_LOG_NONE since we
    // still want to abort.
    return min_severity_ ? min_severity_->load(std::memory_order_relaxed) : FUCHSIA_LOG_FATAL;
  }

  // Flushes the buffer to the logger. If the logger is configured with tags, it must not be
  // finalised; the tags will be added.
  zx::result<> FlushBuffer(LogBuffer& buffer) const;

  // Calls `callback` for each configured tag.
  void ForEachTag(fit::inline_function<void(const std::string&)> callback) const;

 private:
  friend class internal::Logger;

  class Impl;

  explicit Logger(std::shared_ptr<Impl> impl);

  zx::result<> FlushSpan(cpp20::span<const uint8_t> span) const;

  std::shared_ptr<Impl> impl_;
  std::atomic<FuchsiaLogSeverity>* min_severity_ = nullptr;
};

}  // namespace fuchsia_logging

#endif

#endif  // LIB_SYSLOG_STRUCTURED_BACKEND_CPP_LOGGER_H_
