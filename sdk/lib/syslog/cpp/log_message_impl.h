// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_CPP_LOG_MESSAGE_IMPL_H_
#define LIB_SYSLOG_CPP_LOG_MESSAGE_IMPL_H_

#include <lib/syslog/cpp/log_level.h>
#include <zircon/availability.h>
#include <zircon/types.h>

#include <cstdint>
#include <optional>
#ifdef __Fuchsia__
#include <lib/syslog/cpp/logging_backend_fuchsia_globals.h>
#include <lib/syslog/structured_backend/cpp/fuchsia_syslog.h>
#include <lib/syslog/structured_backend/cpp/log_buffer.h>
#else
#include <lib/syslog/cpp/host/log_buffer.h>
#endif
#include <lib/zx/result.h>

#include <atomic>
#include <limits>
#include <sstream>
#include <type_traits>

namespace fuchsia_logging {

/// Constructs a LogBuffer
class LogBufferBuilder {
 public:
  explicit LogBufferBuilder(fuchsia_logging::RawLogSeverity severity) : severity_(severity) {}

  /// Sets the file name and line number for the log message
  LogBufferBuilder& WithFile(std::string_view file, unsigned int line) {
    file_name_ = file;
    line_ = line;
    return *this;
  }

  /// Sets the condition for the log message
  /// This is used by test frameworks that want
  /// to print an assertion message when a test fails.
  /// This prepends the string "Check failed: "<<condition<<". "
  /// to whatever message the user passes.
  LogBufferBuilder& WithCondition(std::string_view condition) {
    condition_ = condition;
    return *this;
  }

  /// Sets the message for the log message
  LogBufferBuilder& WithMsg(std::string_view msg) {
    msg_ = msg;
    return *this;
  }

#if defined(__Fuchsia__) && FUCHSIA_API_LEVEL_LESS_THAN(29)
  /// Sets the socket for the log message
  LogBufferBuilder& WithSocket(zx_handle_t socket) {
    socket_ = socket;
    return *this;
  }
#endif
  /// Builds the LogBuffer
  fuchsia_logging::LogBuffer Build();

 private:
  std::optional<std::string_view> file_name_;
  unsigned int line_ = 0;
  std::optional<std::string_view> msg_;
  std::optional<std::string_view> condition_;
#if defined(__Fuchsia__) && FUCHSIA_API_LEVEL_LESS_THAN(29)
  zx_handle_t socket_ = ZX_HANDLE_INVALID;
#endif
  fuchsia_logging::RawLogSeverity severity_;
};

class LogMessageVoidify final {
 public:
  void operator&(std::ostream&) {}
};

#if !defined(__Fuchsia__) || FUCHSIA_API_LEVEL_AT_LEAST(29)
// Flushes `buffer` to the global logger.
zx::result<> FlushToGlobalLogger(LogBuffer& buffer);
#endif

namespace internal {

// A null-safe wrapper around std::optional<std::string_view>
//
// This class is used to represent a string that may be nullptr. It is used
// to avoid the need to check for nullptr before passing a string to the
// syslog macros.
//
// This class is implicitly convertible to std::optional<std::string_view>.
// NOLINT is used as implicit conversions are intentional here.
class NullSafeStringView final {
 public:
  //  Constructs a NullSafeStringView from a std::string_view.
  constexpr NullSafeStringView(std::string_view string_view)
      : string_view_(string_view) {}  // NOLINT

  // Constructs a NullSafeStringView from a nullptr.
  constexpr NullSafeStringView(std::nullptr_t) : string_view_(std::nullopt) {}  // NOLINT

  constexpr NullSafeStringView(const NullSafeStringView&) = default;

  // Constructs a NullSafeStringView from a const char* which may be nullptr.
  // string Nullable string to construct from.
  constexpr NullSafeStringView(const char* input) {  // NOLINT
    if (!input) {
      string_view_ = std::nullopt;
    } else {
      string_view_ = std::string_view(input);
    }
  }

  // Creates a NullSafeStringView fro, an optional<std::string_view>.
  // This is not a constructor to prevent accidental misuse.
  static NullSafeStringView CreateFromOptional(std::optional<std::string_view> string_view) {
    if (!string_view) {
      return NullSafeStringView(nullptr);
    }
    return NullSafeStringView(*string_view);
  }

  // Constructs a NullSafeStringView from an std::string.
  constexpr NullSafeStringView(const std::string& input) : string_view_(input) {}  // NOLINT

  // Converts this NullSafeStringView to a std::optional<std::string_view>.
  constexpr operator std::optional<std::string_view>() const { return string_view_; }  // NOLINT
 private:
  std::optional<std::string_view> string_view_;
};

template <typename Msg, typename... Args>
void WriteStructuredLog(fuchsia_logging::LogSeverity severity, const char* file, int line, Msg msg,
                        Args... args) {
  LogBufferBuilder builder(severity);
  if (file) {
    builder.WithFile(file, line);
  }
  // If nullptr or a const char* that is nullptr is passed, ignore it.
  if constexpr (std::is_convertible_v<Msg, const char*>) {
    if (const char* msg_str = msg) {
      builder.WithMsg(msg_str);
    }
  } else {
    // NOTE: If you see build errors on this line, it might be because of a Clang issue: fix
    // previous build errors and then try again.
    builder.WithMsg(msg);
  }
  auto buffer = builder.Build();
  (buffer.Encode(args), ...);
#if defined(__Fuchsia__) && FUCHSIA_API_LEVEL_LESS_THAN(29)
  buffer.Flush();
#else
  [[maybe_unused]] zx::result<> result = FlushToGlobalLogger(buffer);
#endif
}

}  // namespace internal

class LogMessage final {
 public:
  LogMessage(RawLogSeverity severity, const char* file, int line, const char* condition,
             const char* tag
#if defined(__Fuchsia__)
             ,
             zx_status_t status = std::numeric_limits<zx_status_t>::max()
#endif
  );
  ~LogMessage();

  std::ostream& stream() { return stream_; }

 private:
  std::ostringstream stream_;
  const RawLogSeverity severity_;
  const char* file_;
  const int line_;
  const char* condition_;
  const char* tag_;
#if defined(__Fuchsia__)
  const zx_status_t status_;
#endif
};

// LogFirstNState is used by the macro FX_LOGS_FIRST_N below.
class LogFirstNState final {
 public:
  bool ShouldLog(uint32_t n);

 private:
  std::atomic<uint32_t> counter_{0};
};

RawLogSeverity GetMinLogSeverity();

#if defined(__Fuchsia__) && FUCHSIA_API_LEVEL_AT_LEAST(29)
inline RawLogSeverity GetMinLogSeverity() { return internal::FuchsiaLogGetGlobalMinSeverity(); }
#endif

/// Returns true if |severity| is at or above the current minimum log level.
/// LOG_FATAL and above is always true.
inline bool IsSeverityEnabled(RawLogSeverity severity) { return severity >= GetMinLogSeverity(); }

}  // namespace fuchsia_logging

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

namespace syslog_runtime {

template <typename K, typename V>
using KeyValue = ::fuchsia_logging::KeyValue<K, V>;
using LogBuffer = ::fuchsia_logging::LogBuffer;
using LogBufferBuilder = ::fuchsia_logging::LogBufferBuilder;

}  // namespace syslog_runtime

#endif

#endif  // LIB_SYSLOG_CPP_LOG_MESSAGE_IMPL_H_
