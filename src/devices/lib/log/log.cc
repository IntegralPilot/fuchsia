// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "log.h"

#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <fidl/fuchsia.logger/cpp/wire_types.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/zx/channel.h>
#include <zircon/process.h>
#include <zircon/processargs.h>

#include <fbl/no_destructor.h>

namespace fdf_log {
namespace {
const char* StripPath(const char* path) {
  auto p = strrchr(path, '/');
  if (p) {
    return p + 1;
  }
  return path;
}

const char* StripDots(const char* path) {
  while (strncmp(path, "../", 3) == 0) {
    path += 3;
  }
  return path;
}

const char* StripFile(const char* file, FuchsiaLogSeverity severity) {
  return severity > FUCHSIA_LOG_INFO ? StripDots(file) : StripPath(file);
}

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

Logger& GetOrCreateLogger() {
  static fbl::NoDestructor<Logger> logger = [] {
    zx::channel logger_request, logger_client;
    zx::channel::create(0, &logger_request, &logger_client);
    fdio_service_connect("/svc/fuchsia.logger.LogSink", logger_request.release());
    zx::socket remote, local;
    zx::socket::create(ZX_SOCKET_DATAGRAM, &remote, &local);
    fidl::ClientEnd<fuchsia_logger::LogSink> log_sink{std::move(logger_client)};
    auto status = fidl::WireCall(log_sink)->ConnectStructured(std::move(remote));
    if (!status.ok()) {
      ZX_PANIC("Failed to create logger: %s", status.status_string());
    }
    return Logger(GetKoid(zx_process_self()), std::move(local));
  }();
  return *logger;
}

#else

Logger CreateLogger(fuchsia_logging::RawLogSettings* settings) {
  zx_koid_t pid = GetKoid(zx_process_self());

  zx::channel log_sink;
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  log_sink = zx::channel{zx_take_startup_handle(PA_LOG_SINK)};
#endif

  if (!log_sink.is_valid()) {
    auto client_end = component::Connect<fuchsia_logger::LogSink>();
    if (client_end.is_ok()) {
      log_sink = client_end->TakeChannel();
    }
  }

  if (log_sink.is_valid()) {
    std::optional<fuchsia_logging::RawLogSettings> settings_storage;
    if (!settings) {
      settings = &settings_storage.emplace();
    }
    settings->log_sink = log_sink.release();
    auto logger = fuchsia_logging::Logger::Create(*settings);
    if (logger.is_ok()) {
      return Logger(pid, *std::move(logger));
    }
  }
  return Logger(pid, {});
}

Logger& GetOrCreateLogger(fuchsia_logging::RawLogSettings* settings = nullptr) {
  bool called = false;
  static fbl::NoDestructor<Logger> logger = [called, settings]() mutable {
    called = true;
    return CreateLogger(settings);
  }();
  if (!called && settings) {
    *logger = CreateLogger(settings);
  }
  return *logger;
}

#endif

zx_koid_t GetCurrentThreadKoid() { return GetKoid(zx_thread_self()); }

thread_local zx_koid_t tid = GetCurrentThreadKoid();

}  // namespace

namespace internal {
FuchsiaLogSeverity severity_from_verbosity(uint8_t verbosity) {
  // verbosity scale sits in the interstitial space between INFO and DEBUG
  FuchsiaLogSeverity severity = FUCHSIA_LOG_INFO - (verbosity * FUCHSIA_LOG_VERBOSITY_STEP_SIZE);
  if (severity < FUCHSIA_LOG_DEBUG + 1) {
    return FUCHSIA_LOG_DEBUG + 1;
  }
  return severity;
}

void log_with_source(Logger& logger, FuchsiaLogSeverity severity, const char* tag, const char* file,
                     int line, const char* format, ...) {
  va_list args;
  va_start(args, format);
  logger.VLogWrite(severity, tag, format, args, file, line);
  va_end(args);
}

const char* SeverityToString(FuchsiaLogSeverity severity) {
  switch (severity) {
    case FUCHSIA_LOG_TRACE:
      return "TRACE";
    case FUCHSIA_LOG_DEBUG:
      return "DEBUG";
    case FUCHSIA_LOG_INFO:
      return "INFO";
    case FUCHSIA_LOG_WARNING:
      return "WARN";
    case FUCHSIA_LOG_ERROR:
      return "ERROR";
    case FUCHSIA_LOG_FATAL:
      return "FATAL";
    default:
      return "";
  }
}

}  // namespace internal

void Logger::VLogWrite(FuchsiaLogSeverity severity, const char* tag, const char* msg, va_list args,
                       const char* file, uint32_t line) const {
  if (severity < GetSeverity()) {
    return;
  }
  if (use_stdout_) {
    // We rely on line buffering to ensure this is a single syscall.
    printf("[driver_manager.cm]: %s: ", internal::SeverityToString(severity));
    vprintf(msg, args);
    fputc('\n', stdout);
    return;
  }
  fuchsia_logging::LogBuffer buffer;
  constexpr size_t kFormatStringLength = 1024;
  char fmt_string[kFormatStringLength];
  fmt_string[kFormatStringLength - 1] = 0;
  int n = kFormatStringLength;
  // Format
  // Number of bytes written not including null terminator
  int count = 0;
  count = vsnprintf(fmt_string, n, msg, args) + 1;

  if (count >= n) {
    // truncated
    constexpr char kEllipsis[] = "...";
    constexpr size_t kEllipsisSize = sizeof(kEllipsis);
    snprintf(fmt_string + kFormatStringLength - 1 - kEllipsisSize, kEllipsisSize, kEllipsis);
  }

  if (file) {
    file = StripFile(file, severity);
  }
  BeginRecord(buffer, severity, file, line, fmt_string);
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  for (const auto& global_tag : global_tags_) {
    buffer.WriteKeyValue("tag", global_tag);
  }
#endif
  if (tag) {
    buffer.WriteKeyValue("tag", tag);
  }
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  FlushRecord(buffer, severity);
#else
  [[maybe_unused]] zx::result<> result = logger_.FlushBuffer(buffer);
#endif
}

void Logger::BeginRecord(fuchsia_logging::LogBuffer& buffer, FuchsiaLogSeverity severity,
                         std::optional<std::string_view> file_name, unsigned int line,
                         std::optional<std::string_view> msg) const {
  buffer.BeginRecord(severity, file_name, line, msg,
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
                     socket_.borrow(),
#endif
                     0, pid_, GetCurrentThread());
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
void Logger::FlushRecord(fuchsia_logging::LogBuffer& buffer, FuchsiaLogSeverity severity) const {
  buffer.FlushRecord();
}
#endif

zx_koid_t GetCurrentThread() { return tid; }

Logger& GetLogger() { return GetOrCreateLogger(); }

#if FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
void InitGlobalLogger(std::span<const char*> tags, FuchsiaLogSeverity severity) {
  fuchsia_logging::RawLogSettings settings{
      .min_log_level = severity,
      .tags = tags.data(),
      .tags_count = tags.size(),
  };
  GetOrCreateLogger(&settings);
}
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD) && __cplusplus >= 202002L
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

void Logger::LogInternal(FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
                         std::string_view fmt, std::format_args args) {
  // The complementary cases are handled directly in the header, to rely on
  // std::format_string template pack.
  ZX_ASSERT(severity >= GetSeverity());
  ZX_ASSERT(!use_stdout_);

  constexpr size_t kFormatStringLength = 1024;
  std::array<char, kFormatStringLength> fmt_buffer;
  size_t actual_size = 0;

  std::vformat_to(array_output_iterator(fmt_buffer, actual_size), fmt, args);
  if (actual_size == 0) {
    return;
  }

  if (actual_size >= kFormatStringLength) {
    // truncated
    constexpr char kEllipsis[] = "...";
    constexpr size_t kEllipsisSize = sizeof(kEllipsis);
    snprintf(fmt_buffer.data() + kFormatStringLength - kEllipsisSize, kEllipsisSize, kEllipsis);
  }
  fmt_buffer[kFormatStringLength - 1] = 0;

  file = StripFile(file, severity);
  fuchsia_logging::LogBuffer buffer;
  BeginRecord(buffer, severity, file, line,
              std::string_view(fmt_buffer.data(), std::min(actual_size, kFormatStringLength)));
  if (tag) {
    buffer.WriteKeyValue("tag", tag);
  }
  [[maybe_unused]] zx::result<> result = logger_.FlushBuffer(buffer);

  if (severity == FUCHSIA_LOG_FATAL) {
    abort();
  }
}
#endif

}  // namespace fdf_log
