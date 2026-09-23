// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_LOGGING_CPP_LOGGER_H_
#define LIB_DRIVER_LOGGING_CPP_LOGGER_H_

#include <lib/stdcompat/span.h>
#include <lib/syslog/structured_backend/fuchsia_syslog.h>
#include <lib/zx/result.h>

#include <atomic>
#include <memory>
#include <string>
#include <string_view>

#if defined(__Fuchsia__)
#define HOST_LOGGING 0
#else  // defined(__Fuchsia__)
#define HOST_LOGGING 1
#endif  // defined(__Fuchsia__)

#if !HOST_LOGGING
#include <fidl/fuchsia.logger/cpp/markers.h>
#include <fidl/fuchsia.logger/cpp/wire_types.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/syslog/structured_backend/cpp/log_buffer.h>
#include <lib/syslog/structured_backend/cpp/logger.h>
#include <lib/zx/socket.h>
#endif  // !HOST_LOGGING

#include <zircon/availability.h>

#include <optional>

#if __cplusplus >= 202002L
#include <format>
#include <source_location>

#include "internal/panic.h"
#endif  // (FUCHSIA_API_LEVEL_AT_LEAST(HEAD) || HOST_LOGGING) && __cplusplus >= 202002L

#define FDF_LOGL(severity, logger, msg...) \
  (logger).logf((FUCHSIA_LOG_##severity), nullptr, __FILE__, __LINE__, msg)

#define FDF_LOG(severity, msg...) FDF_LOGL(severity, *fdf::Logger::GlobalInstance(), msg)

#if __cplusplus >= 202002L

#define FDF_ASSERT(x)                                                     \
  do {                                                                    \
    if (x) [[likely]] {                                                   \
      break;                                                              \
    } else {                                                              \
      fdf::panic("ASSERT FAILED at ({}:{}): {}", __FILE__, __LINE__, #x); \
    }                                                                     \
  } while (0)

#define FDF_ASSERT_MSG(x, msg, msgargs...)                                               \
  do {                                                                                   \
    if (x) [[likely]] {                                                                  \
      break;                                                                             \
    } else {                                                                             \
      fdf::panic("ASSERT FAILED at ({}:{}): {}" msg, __FILE__, __LINE__, #x, ##msgargs); \
    }                                                                                    \
  } while (0)
#endif  // (FUCHSIA_API_LEVEL_AT_LEAST(HEAD) || HOST_LOGGING) && __cplusplus >= 202002L

namespace fdf {

enum LogSeverity : uint8_t {
  TRACE = FUCHSIA_LOG_TRACE,
  DEBUG = FUCHSIA_LOG_DEBUG,
  INFO = FUCHSIA_LOG_INFO,
  WARN = FUCHSIA_LOG_WARNING,
  ERROR = FUCHSIA_LOG_ERROR,
  FATAL = FUCHSIA_LOG_FATAL,
};

// Provides a driver's logger.
class Logger final {
 public:
  // Creates a logger with a given |name|, which will only send logs that are of
  // at least |min_severity|.
  //
  // |dispatcher| must be single threaded or synchornized. Create must be called from the context of
  // the |dispatcher|.
  //
  // If we fail to connect to LogSink, or if there's any error the returned logger will be no-op.
#if !HOST_LOGGING
  static std::unique_ptr<Logger> Create2(
      const Namespace& ns, async_dispatcher_t* dispatcher, std::string_view name,
      FuchsiaLogSeverity min_severity = FUCHSIA_LOG_INFO
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
      ,
      bool wait_for_initial_interest = true
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
      ,
      std::optional<fidl::ClientEnd<fuchsia_logger::LogSink>> maybe_log_sink = std::nullopt
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
  );

  static zx::result<std::unique_ptr<Logger>> Create(
      const Namespace& ns, async_dispatcher_t* dispatcher, std::string_view name,
      FuchsiaLogSeverity min_severity = FUCHSIA_LOG_INFO
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
      ,
      bool wait_for_initial_interest = true
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
      ,
      std::optional<fidl::ClientEnd<fuchsia_logger::LogSink>> maybe_log_sink = std::nullopt
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
      )
      ZX_DEPRECATED_SINCE(1, 24, "Use Create2 which will return a no-op logger instead of failing");
#endif  // !HOST_LOGGING

  static Logger* GlobalInstance();
  static void SetGlobalInstance(Logger*);
  static bool HasGlobalInstance();

  // A no-op logger.
  Logger() = default;

#if !HOST_LOGGING
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  Logger(std::string_view name, FuchsiaLogSeverity min_severity, zx::socket socket,
         fidl::WireClient<fuchsia_logger::LogSink> log_sink)
      : tag_(name),
        socket_(std::move(socket)),
        default_severity_(min_severity),
        severity_(min_severity),
        log_sink_(std::move(log_sink)) {}
#else   // FUCHSIA_API_LEVEL_LESS_THAN(29)
  explicit Logger(fuchsia_logging::Logger logger) : logger_(std::move(logger)) {}
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#else   // !HOST_LOGGING
  Logger(std::string_view name, FuchsiaLogSeverity min_severity)
      : tag_(name), severity_(min_severity) {}
#endif  // !HOST_LOGGING

  ~Logger();

  // Retrieves the number of dropped logs and resets it
  uint32_t GetAndResetDropped();

  FuchsiaLogSeverity GetSeverity();

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  void SetSeverity(FuchsiaLogSeverity severity);
#endif

  void logf(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
            const char* msg, ...) __PRINTFLIKE(6, 7);
  void logvf(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
             const char* msg, va_list args);
  void logvf(FuchsiaLogSeverity severity, cpp20::span<std::string> tags, const char* file, int line,
             const char* msg, va_list args);

#if __cplusplus >= 202002L
  struct SeverityAndSourceLocation {
    FuchsiaLogSeverity severity;
    std::source_location loc;

    consteval SeverityAndSourceLocation(
        LogSeverity severity, const std::source_location& loc = std::source_location::current())
        : severity(severity), loc(loc) {}
  };

  template <typename... Args>
  void log(SeverityAndSourceLocation sasl, std::format_string<Args...> fmt, Args&&... args) {
    log(nullptr, sasl.severity, sasl.loc.file_name(), sasl.loc.line(), fmt,
        std::forward<Args>(args)...);
  }

  template <typename... Args>
  void log(FuchsiaLogSeverity severity, const std::source_location& loc,
           std::format_string<Args...> fmt, Args&&... args) {
    log(nullptr, severity, loc.file_name(), loc.line(), fmt, std::forward<Args>(args)...);
  }

  template <typename... Args>
  void log(const char* tag, FuchsiaLogSeverity severity, const char* file, int line,
           std::format_string<Args...> fmt, Args&&... args) {
    vlog(severity, tag, file, line, fmt.get(), std::make_format_args(args...));
  }

  void vlog(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
            std::string_view fmt, std::format_args args);
#endif

#if !HOST_LOGGING
  // Begins a structured logging record. You probably don't want to call
  // this directly.
  void BeginRecord(fuchsia_logging::LogBuffer& buffer, FuchsiaLogSeverity severity,
                   std::optional<std::string_view> file_name, unsigned int line,
                   std::optional<std::string_view> message, uint32_t dropped);

  // Sends a log record to the backend. You probably don't want to call this directly.
  // This call also increments dropped_logs_, which is why we don't call FlushRecord
  // on LogBuffer directly.
  bool FlushRecord(fuchsia_logging::LogBuffer& buffer, uint32_t dropped);

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  bool IsNoOp() const { return !socket_.is_valid(); }
#else
  bool IsNoOp() const { return !logger_.IsValid(); }
#endif
#else   // !HOST_LOGGING
  static bool IsNoOp() { return false; }
#endif  // !HOST_LOGGING

 private:
  Logger(const Logger& other) = delete;
  Logger& operator=(const Logger& other) = delete;

#if !HOST_LOGGING
  static zx::result<std::unique_ptr<Logger>> MaybeCreate(
      const Namespace& ns, async_dispatcher_t* dispatcher, std::string_view name,
      FuchsiaLogSeverity min_severity = FUCHSIA_LOG_INFO
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
      ,
      bool wait_for_initial_interest = true
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
      ,
      std::optional<fidl::ClientEnd<fuchsia_logger::LogSink>> maybe_log_sink = std::nullopt
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
  );
#endif  // !HOST_LOGGING

  static std::unique_ptr<Logger> NoOp();

#if !HOST_LOGGING
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(27)
  void HandleInterest(fuchsia_diagnostics_types::wire::Interest interest);
#else   // FUCHSIA_API_LEVEL_AT_LEAST(27)
  void HandleInterest(fuchsia_diagnostics::wire::Interest interest);
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(27)

  void OnInterestChange(
      fidl::WireUnownedResult<fuchsia_logger::LogSink::WaitForInterestChange>& result);

  // For thread-safety these members should be read-only.
  const std::string tag_;
#if !HOST_LOGGING
  const zx::socket socket_;
  const FuchsiaLogSeverity default_severity_ = FUCHSIA_LOG_INFO;
#endif  // !HOST_LOGGING
  // Messages below this won't be logged. This field is thread-safe.
  std::atomic<FuchsiaLogSeverity> severity_ = FUCHSIA_LOG_INFO;

  // Used to learn about changes in severity.
  fidl::WireClient<fuchsia_logger::LogSink> log_sink_;
#else   // FUCHSIA_API_LEVEL_AT_LEAST(29)
  const fuchsia_logging::Logger logger_;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(29)
#else   // !HOST_LOGGING
  const std::string tag_;
  std::atomic<FuchsiaLogSeverity> severity_ = FUCHSIA_LOG_INFO;
#endif  // !HOST_LOGGING

  // Dropped log count. This is thread-safe and is reset on success.
  std::atomic<uint32_t> dropped_logs_ = 0;
};

#if __cplusplus >= 202002L
// Use template type deduction to allow us to get source location while using variadic templates.
template <typename... Args>
struct trace {
  trace(std::format_string<Args...> fmt, Args&&... args,
        const std::source_location& loc = std::source_location::current()) {
    fdf::Logger::GlobalInstance()->log(FUCHSIA_LOG_TRACE, loc, fmt, std::forward<Args>(args)...);
  }
};

template <typename... Args>
trace(std::format_string<Args...>, Args&&...) -> trace<Args...>;

template <typename... Args>
struct debug {
  debug(std::format_string<Args...> fmt, Args&&... args,
        const std::source_location& loc = std::source_location::current()) {
    fdf::Logger::GlobalInstance()->log(FUCHSIA_LOG_DEBUG, loc, fmt, std::forward<Args>(args)...);
  }
};

template <typename... Args>
debug(std::format_string<Args...>, Args&&...) -> debug<Args...>;

template <typename... Args>
struct info {
  info(std::format_string<Args...> fmt, Args&&... args,
       const std::source_location& loc = std::source_location::current()) {
    fdf::Logger::GlobalInstance()->log(FUCHSIA_LOG_INFO, loc, fmt, std::forward<Args>(args)...);
  }
};

template <typename... Args>
info(std::format_string<Args...>, Args&&...) -> info<Args...>;

template <typename... Args>
struct warn {
  warn(std::format_string<Args...> fmt, Args&&... args,
       const std::source_location& loc = std::source_location::current()) {
    fdf::Logger::GlobalInstance()->log(FUCHSIA_LOG_WARNING, loc, fmt, std::forward<Args>(args)...);
  }
};

template <typename... Args>
warn(std::format_string<Args...>, Args&&...) -> warn<Args...>;

template <typename... Args>
struct error {
  error(std::format_string<Args...> fmt, Args&&... args,
        const std::source_location& loc = std::source_location::current()) {
    fdf::Logger::GlobalInstance()->log(FUCHSIA_LOG_ERROR, loc, fmt, std::forward<Args>(args)...);
  }
};

template <typename... Args>
error(std::format_string<Args...>, Args&&...) -> error<Args...>;

template <typename... Args>
struct fatal {
  fatal(std::format_string<Args...> fmt, Args&&... args,
        const std::source_location& loc = std::source_location::current()) {
    fdf::Logger::GlobalInstance()->log(FUCHSIA_LOG_ERROR, loc, fmt, std::forward<Args>(args)...);
  }
};

template <typename... Args>
fatal(std::format_string<Args...>, Args&&...) -> fatal<Args...>;

template <typename... Args>
void panic(std::format_string<Args...> fmt, Args&&... args) {
  fdf_internal::vpanic(fmt.get(), std::make_format_args(args...));
}
#endif

}  // namespace fdf

#endif  // LIB_DRIVER_LOGGING_CPP_LOGGER_H_
