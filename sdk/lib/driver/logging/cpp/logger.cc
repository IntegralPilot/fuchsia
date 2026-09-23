// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>

#if !HOST_LOGGING
#include <fidl/fuchsia.logger/cpp/wire_messaging.h>
#include <lib/fdio/directory.h>
#include <zircon/process.h>
#else  // !HOST_LOGGING
// Host includes
#include <lib/syslog/cpp/host/log_buffer.h>
#include <lib/syslog/cpp/log_message_impl.h>  // nogncheck
#include <zircon/assert.h>

#include <cstdio>
#include <iostream>
#include <vector>
#endif  // !HOST_LOGGING

#include <algorithm>
#include <array>
#include <cstdarg>
#include <iterator>
#include <optional>

namespace flog = ::fuchsia_logging;

namespace fdf {

namespace {
std::atomic<Logger*> g_instance = nullptr;

#if !HOST_LOGGING
#if FUCHSIA_API_LEVEL_AT_LEAST(27)
using FidlSeverity = fuchsia_diagnostics_types::wire::Severity;
using FidlInterest = fuchsia_diagnostics_types::wire::Interest;
#else   // FUCHSIA_API_LEVEL_AT_LEAST(27)
using FidlSeverity = fuchsia_diagnostics::wire::Severity;
using FidlInterest = fuchsia_diagnostics::wire::Interest;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(27)

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}
#endif  // !HOST_LOGGING

}  // namespace

#if !HOST_LOGGING
bool Logger::FlushRecord(flog::LogBuffer& buffer, uint32_t dropped) {
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  if (!buffer.FlushRecord()) {
    dropped_logs_.fetch_add(dropped + 1, std::memory_order_relaxed);
    return false;
  }
#else   // FUCHSIA_API_LEVEL_LESS_THAN(29)
  if (logger_.FlushBuffer(buffer).is_error()) {
    dropped_logs_.fetch_add(dropped + 1, std::memory_order_relaxed);
    return false;
  }
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
  return true;
}

void Logger::BeginRecord(flog::LogBuffer& buffer, FuchsiaLogSeverity severity,
                         std::optional<std::string_view> file_name, unsigned int line,
                         std::optional<std::string_view> message, uint32_t dropped) {
  static zx_koid_t pid = GetKoid(zx_process_self());
  static thread_local zx_koid_t tid = GetKoid(zx_thread_self());
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  buffer.BeginRecord(severity, file_name, line, message, socket_.borrow(), dropped, pid, tid);
  buffer.WriteKeyValue("tag", "driver");
  buffer.WriteKeyValue("tag", tag_);
#else   // FUCHSIA_API_LEVEL_LESS_THAN(29)
  buffer.BeginRecord(severity, file_name, line, message, dropped, pid, tid);
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
}

std::unique_ptr<Logger> Logger::Create2(
    const Namespace& ns, async_dispatcher_t* dispatcher, std::string_view name,
    FuchsiaLogSeverity min_severity
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
    ,
    bool wait_for_initial_interest
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    ,
    std::optional<fidl::ClientEnd<fuchsia_logger::LogSink>> maybe_log_sink
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
) {
  auto result = Logger::MaybeCreate(ns, dispatcher, name, min_severity
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
                                    ,
                                    wait_for_initial_interest
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
                                    ,
                                    std::move(maybe_log_sink)
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
  );
  if (!result.is_ok()) {
    return Logger::NoOp();
  }
  return std::move(result.value());
}

zx::result<std::unique_ptr<Logger>> Logger::Create(
    const Namespace& ns, async_dispatcher_t* dispatcher, std::string_view name,
    FuchsiaLogSeverity min_severity
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
    ,
    bool wait_for_initial_interest
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    ,
    std::optional<fidl::ClientEnd<fuchsia_logger::LogSink>> maybe_log_sink
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
) {
  auto result = Logger::MaybeCreate(ns, dispatcher, name, min_severity
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
                                    ,
                                    wait_for_initial_interest
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
                                    ,
                                    std::move(maybe_log_sink)
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
  );
  if (!result.is_ok()) {
    return zx::ok(Logger::NoOp());
  }
  return result;
}

zx::result<std::unique_ptr<Logger>> Logger::MaybeCreate(
    const Namespace& ns, async_dispatcher_t* dispatcher, std::string_view name,
    FuchsiaLogSeverity min_severity
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
    ,
    bool wait_for_initial_interest
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    ,
    std::optional<fidl::ClientEnd<fuchsia_logger::LogSink>> maybe_log_sink
#endif
) {
  zx::channel log_sink;
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  if (maybe_log_sink.has_value()) {
    log_sink = maybe_log_sink->TakeChannel();
  } else {
#endif
    auto ns_result = ns.Connect<fuchsia_logger::LogSink>();
    if (ns_result.is_error()) {
      return ns_result.take_error();
    }
    log_sink = ns_result->TakeChannel();
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  }
#endif

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  zx::socket client_end, server_end;
  zx_status_t status = zx::socket::create(ZX_SOCKET_DATAGRAM, &client_end, &server_end);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  fidl::WireClient<fuchsia_logger::LogSink> log_sink_client(
      fidl::ClientEnd<fuchsia_logger::LogSink>(std::move(log_sink)), dispatcher);
  auto sink_result = log_sink_client->ConnectStructured(std::move(server_end));
  if (!sink_result.ok()) {
    return zx::error(sink_result.status());
  }

  auto logger = std::make_unique<Logger>(name, min_severity, std::move(client_end),
                                         std::move(log_sink_client));

  if (wait_for_initial_interest) {
    auto interest_result = logger->log_sink_.sync()->WaitForInterestChange();
    if (!interest_result.ok()) {
      return zx::error(interest_result.status());
    }
    // We are guanteed to not call this twice so we can ignore the application error.
    logger->HandleInterest(interest_result->value()->data);
  }

  logger->log_sink_->WaitForInterestChange().Then(
      fit::bind_member(logger.get(), &Logger::OnInterestChange));

  return zx::ok(std::move(logger));
#else   // FUCHSIA_API_LEVEL_LESS_THAN(29)
  std::string name_str(name);
  const char* tags[] = {"driver", name_str.c_str()};
  if (auto logger = flog::Logger::Create(flog::RawLogSettings{
          .min_log_level = min_severity,
          .log_sink = log_sink.release(),
          .tags = tags,
          .tags_count = 2,
          .dispatcher = dispatcher,
      });
      logger.is_error()) {
    return logger.take_error();
  } else {
    return zx::ok(std::make_unique<Logger>(*std::move(logger)));
  }
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
void Logger::HandleInterest(FidlInterest interest) {
  if (interest.has_min_severity()) {
    switch (interest.min_severity()) {
      case FidlSeverity::kTrace:
        severity_ = FUCHSIA_LOG_TRACE;
        return;
      case FidlSeverity::kDebug:
        severity_ = FUCHSIA_LOG_DEBUG;
        return;
      case FidlSeverity::kInfo:
        severity_ = FUCHSIA_LOG_INFO;
        return;
      case FidlSeverity::kWarn:
        severity_ = FUCHSIA_LOG_WARNING;
        return;
      case FidlSeverity::kError:
        severity_ = FUCHSIA_LOG_ERROR;
        return;
      case FidlSeverity::kFatal:
        severity_ = FUCHSIA_LOG_FATAL;
        return;
#if FUCHSIA_API_LEVEL_AT_LEAST(27)
      default:
        severity_ = FUCHSIA_LOG_INFO;
        return;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(27)
    }
  } else {
    severity_ = default_severity_;
  }
}

void Logger::OnInterestChange(
    fidl::WireUnownedResult<fuchsia_logger::LogSink::WaitForInterestChange>& result) {
  if (result.ok()) {
    HandleInterest(result->value()->data);
    log_sink_->WaitForInterestChange().Then(fit::bind_member(this, &Logger::OnInterestChange));
  }
}
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
#endif  // !HOST_LOGGING

std::unique_ptr<Logger> Logger::NoOp() { return std::make_unique<Logger>(); }

Logger* Logger::GlobalInstance() {
  ZX_DEBUG_ASSERT(HasGlobalInstance());
  return g_instance.load();
}

void Logger::SetGlobalInstance(Logger* logger) { g_instance = logger; }

bool Logger::HasGlobalInstance() { return g_instance != nullptr; }

Logger::~Logger() = default;

uint32_t Logger::GetAndResetDropped() {
  return dropped_logs_.exchange(0, std::memory_order_relaxed);
}

FuchsiaLogSeverity Logger::GetSeverity() {
#if FUCHSIA_API_LEVEL_LESS_THAN(29) || HOST_LOGGING
  return severity_.load(std::memory_order_relaxed);
#else
  return logger_.GetMinSeverity();
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29) || HOST_LOGGING
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29) || HOST_LOGGING
void Logger::SetSeverity(FuchsiaLogSeverity severity) { severity_.store(severity); }
#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29) || HOST_LOGGING

void Logger::logf(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
                  const char* msg, ...) {
  va_list args;
  va_start(args, msg);
  logvf(severity, tag, file, line, msg, args);
  va_end(args);
}

namespace {
const char* StripDots(const char* path) {
  while (strncmp(path, "../", 3) == 0) {
    path += 3;
  }
  return path;
}

const char* StripPath(const char* path) {
  auto p = strrchr(path, '/');
  if (p) {
    return p + 1;
  }
  return path;
}

const char* StripFile(const char* file, FuchsiaLogSeverity severity) {
  return severity > FUCHSIA_LOG_INFO ? StripDots(file) : StripPath(file);
}
}  // namespace

void Logger::logvf(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
                   const char* msg, va_list args) {
  if (tag) {
    std::string tag_str(tag);
    logvf(severity, {&tag_str, 1}, file, line, msg, args);
  } else {
    logvf(severity, cpp20::span<std::string>(), file, line, msg, args);
  }
}

#if !HOST_LOGGING
void Logger::logvf(FuchsiaLogSeverity severity, cpp20::span<std::string> tags, const char* file,
                   int line, const char* msg, va_list args) {
  if (!file || line <= 0) {
    // We require a file and line number for printf-style logs.
    return;
  }
  if (severity < GetSeverity()) {
    return;
  }
  uint32_t dropped = dropped_logs_.exchange(0, std::memory_order_relaxed);
  constexpr size_t kFormatStringLength = 1024;
  char fmt_string[kFormatStringLength];
  fmt_string[kFormatStringLength - 1] = 0;
  int n = kFormatStringLength;
  // Format
  // Number of bytes written not including null terminator
  int count = 0;
  count = vsnprintf(fmt_string, n, msg, args) + 1;
  if (count < 0) {
    // Invalid arguments -- we don't support logging empty strings
    // for legacy printf-style messages.
    return;
  }

  if (count >= n) {
    // truncated
    constexpr char kEllipsis[] = "...";
    constexpr size_t kEllipsisSize = sizeof(kEllipsis);
    snprintf(fmt_string + kFormatStringLength - 1 - kEllipsisSize, kEllipsisSize, kEllipsis);
  }

  file = StripFile(file, severity);
  flog::LogBuffer buffer;
  BeginRecord(buffer, severity, file, line, fmt_string, dropped);
  for (const auto& tag : tags) {
    buffer.WriteKeyValue("tag", tag);
  }
  FlushRecord(buffer, dropped);

  if (severity == FUCHSIA_LOG_FATAL) {
    abort();
  }
}
#else   // !HOST_LOGGING
void Logger::logvf(FuchsiaLogSeverity severity, cpp20::span<std::string> tags, const char* file,
                   int line, const char* msg, va_list args) {
  if (!file || line <= 0) {
    return;
  }
  if (severity < GetSeverity()) {
    return;
  }

  constexpr size_t kFormatStringLength = 1024;
  char fmt_string[kFormatStringLength];
  fmt_string[kFormatStringLength - 1] = 0;
  int n = kFormatStringLength;
  int count = vsnprintf(fmt_string, n, msg, args) + 1;
  if (count < 0) {
    return;
  }

  if (count >= n) {
    // truncated
    constexpr char kEllipsis[] = "...";
    constexpr size_t kEllipsisSize = sizeof(kEllipsis);
    snprintf(fmt_string + kFormatStringLength - 1 - kEllipsisSize, kEllipsisSize, kEllipsis);
  }

  file = StripFile(file, severity);

  flog::LogBufferBuilder builder(severity);
  builder.WithFile(file, line);
  builder.WithMsg(fmt_string);
  auto buffer = builder.Build();
  for (const auto& tag : tags) {
    buffer.WriteKeyValue("tag", tag);
  }
  (void)flog::FlushToGlobalLogger(buffer);

  if (severity == FUCHSIA_LOG_FATAL) {
    abort();
  }
}
#endif  // !HOST_LOGGING

#if __cplusplus >= 202002L
namespace {
template <typename T, std::size_t N>
class array_output_iterator {
 public:
  using iterator_category = std::output_iterator_tag;
  using value_type = void;
  using difference_type = std::ptrdiff_t;
  using pointer = void;
  using reference = void;

  explicit array_output_iterator(std::array<T, N>& arr, size_t& actual_size)
      : arr_(&arr), actual_size_(&actual_size) {}

  array_output_iterator(const array_output_iterator&) = default;
  array_output_iterator& operator=(const array_output_iterator&) = default;
  array_output_iterator(array_output_iterator&&) = default;
  array_output_iterator& operator=(array_output_iterator&&) = default;

  array_output_iterator& operator=(const T& value) {
    if (index_ < N) {
      (*arr_)[index_] = value;
    }
    return *this;
  }

  array_output_iterator& operator*() { return *this; }
  array_output_iterator& operator++() {
    ++index_;
    (*actual_size_)++;
    return *this;
  }
  array_output_iterator operator++(int) {
    auto tmp = *this;
    ++index_;
    (*actual_size_)++;
    return tmp;
  }

 private:
  std::array<T, N>* arr_;
  size_t* actual_size_;
  size_t index_ = 0;
};

template <typename T, size_t N>
array_output_iterator(std::array<T, N>&, size_t&) -> array_output_iterator<T, N>;
}  // namespace

void Logger::vlog(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
                  std::string_view fmt, std::format_args args) {
  if (severity < GetSeverity()) {
    return;
  }
  constexpr size_t kFormatStringLength = 1024;
  std::array<char, kFormatStringLength> fmt_buffer;
  size_t actual_size = 0;

  std::vformat_to(array_output_iterator(fmt_buffer, actual_size), fmt, args);
  if (actual_size == 0) {
    return;
  }

#if !HOST_LOGGING
  uint32_t dropped = dropped_logs_.exchange(0, std::memory_order_relaxed);
#endif

  if (actual_size >= kFormatStringLength) {
    // truncated
    constexpr char kEllipsis[] = "...";
    constexpr size_t kEllipsisSize = sizeof(kEllipsis);
    snprintf(fmt_buffer.data() + kFormatStringLength - kEllipsisSize, kEllipsisSize, kEllipsis);
  }
  fmt_buffer[kFormatStringLength - 1] = 0;

  file = StripFile(file, severity);

#if !HOST_LOGGING
  flog::LogBuffer buffer;
  BeginRecord(buffer, severity, file, line,
              std::string_view(fmt_buffer.data(), std::min(actual_size, kFormatStringLength)),
              dropped);
  if (tag) {
    buffer.WriteKeyValue("tag", tag);
  }
  FlushRecord(buffer, dropped);
#else   // !HOST_LOGGING
  // On Host, we use the syslog cpp backend which provides a compatible LogBuffer.
  // We don't support dropped log count on host yet.
  flog::LogBufferBuilder builder(severity);
  builder.WithFile(file, line);
  builder.WithMsg(std::string_view(fmt_buffer.data(), std::min(actual_size, kFormatStringLength)));
  auto buffer = builder.Build();

  if (tag) {
    buffer.WriteKeyValue("tag", tag);
  }
  (void)flog::FlushToGlobalLogger(buffer);
#endif  // !HOST_LOGGING

  if (severity == FUCHSIA_LOG_FATAL) {
    abort();
  }
}
#endif  // (FUCHSIA_API_LEVEL_AT_LEAST(HEAD) || HOST_LOGGING) && __cplusplus >= 202002L

}  // namespace fdf
