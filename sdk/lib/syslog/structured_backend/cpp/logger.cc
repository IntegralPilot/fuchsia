// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/syslog/structured_backend/cpp/logger.h"

#if FUCHSIA_API_LEVEL_AT_LEAST(29)

#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <lib/syslog/structured_backend/cpp/log_connection.h>

#include <vector>

namespace fuchsia_logging {
namespace {
FuchsiaLogSeverity GetSeverityFromInterestChange(
    const fuchsia_logger::wire::LogSinkWaitForInterestChangeResponse& response,
    FuchsiaLogSeverity default_severity) {
  const auto& interest = response.data;
  return interest.has_min_severity() ? static_cast<FuchsiaLogSeverity>(interest.min_severity())
                                     : default_severity;
}
}  // namespace

class Logger::Impl {
 public:
  using OnSeverityChanged = void (*)(void*, FuchsiaLogSeverity);

  static zx::result<std::shared_ptr<Impl>> Create(const RawLogSettings& settings,
                                                  std::atomic<FuchsiaLogSeverity>* min_severity) {
    if (settings.log_sink == ZX_HANDLE_INVALID) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    fidl::ClientEnd<fuchsia_logger::LogSink> client_end;
    client_end = fidl::ClientEnd<fuchsia_logger::LogSink>(zx::channel(settings.log_sink));

    auto result = internal::LogConnection::Create(client_end);
    if (result.is_error()) {
      return result.take_error();
    }

    auto [connection, received_severity] = *std::move(result);

    if (!settings.dispatcher) {
      // If the caller isn't interested in interest updates, we assume the caller doesn't want the
      // initial interest either.
      received_severity = {};
    }

    std::vector<std::string> tags;
    tags.reserve(settings.tags_count);
    for (size_t i = 0; i < settings.tags_count; ++i) {
      tags.push_back(std::string(settings.tags[i]));
    }

    auto impl = std::make_shared<Impl>(settings.min_log_level, min_severity, std::move(connection),
                                       std::move(tags), settings.severity_change_callback,
                                       settings.severity_change_callback_context);
    fidl::WireSharedClient<fuchsia_logger::LogSink> log_sink;
    if (received_severity) {
      impl->HandleInterestChange(*received_severity);
    }

    if (settings.dispatcher) {
      impl->log_sink_.Bind(std::move(client_end), settings.dispatcher);
      PollInterest(impl);
    }

    return zx::ok(std::move(impl));
  }

  Impl(FuchsiaLogSeverity default_severity, std::atomic<FuchsiaLogSeverity>* min_severity,
       internal::LogConnection connection, std::vector<std::string> tags,
       OnSeverityChanged on_severity_changed, void* on_severity_changed_context)
      : default_severity_(default_severity),
        min_severity_(min_severity ? min_severity : &min_severity_storage_),
        connection_(std::move(connection)),
        tags_(std::move(tags)),
        on_severity_changed_(on_severity_changed ? on_severity_changed
                                                 : +[](void*, FuchsiaLogSeverity) {}),
        on_severity_changed_context_(on_severity_changed_context) {
    min_severity_->store(default_severity, std::memory_order_relaxed);
  }

  const std::vector<std::string>& tags() const { return tags_; }

  zx::result<> FlushSpan(cpp20::span<const uint8_t> span) const {
    return connection_.FlushSpan(span);
  }

  std::atomic<uint8_t>& min_severity() { return *min_severity_; }

 private:
  static void PollInterest(const std::shared_ptr<Impl>& impl) {
    impl->log_sink_->WaitForInterestChange().Then(
        [weak = std::weak_ptr(impl)](
            const fidl::BaseWireResult<fuchsia_logger::LogSink::WaitForInterestChange>&
                interest_result) {
          if (auto impl = weak.lock()) {
            if (interest_result.ok() && interest_result->is_ok()) {
              impl->HandleInterestChange(
                  GetSeverityFromInterestChange(***interest_result, impl->default_severity_));
              PollInterest(impl);
            }
          }
        });
  }

  void HandleInterestChange(FuchsiaLogSeverity new_severity) {
    min_severity_->store(new_severity, std::memory_order_relaxed);
    on_severity_changed_(on_severity_changed_context_, new_severity);
  }

  FuchsiaLogSeverity default_severity_ = FUCHSIA_LOG_INFO;
  std::atomic<FuchsiaLogSeverity>* min_severity_;
  // Only used if severity_ not stored externally.
  std::atomic<FuchsiaLogSeverity> min_severity_storage_;
  internal::LogConnection connection_;
  const std::vector<std::string> tags_;
  fidl::WireSharedClient<fuchsia_logger::LogSink> log_sink_;
  OnSeverityChanged on_severity_changed_ = nullptr;
  void* on_severity_changed_context_ = nullptr;
};

zx::result<Logger> Logger::Create(const RawLogSettings& settings,
                                  std::atomic<FuchsiaLogSeverity>* min_severity) {
  auto impl = Impl::Create(settings, min_severity);
  if (impl.is_error()) {
    return impl.take_error();
  }
  return zx::ok(Logger(*std::move(impl)));
}

Logger::Logger(std::shared_ptr<Impl> impl)
    : impl_(std::move(impl)), min_severity_(&impl_->min_severity()) {}

zx::result<> Logger::FlushBuffer(LogBuffer& buffer) const {
  if (!impl_) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  for (const std::string& tag : impl_->tags()) {
    buffer.WriteKeyValue("tag", tag);
  }
  return impl_->FlushSpan(buffer.EndRecord());
}

zx::result<> Logger::FlushSpan(cpp20::span<const uint8_t> span) const {
  if (!impl_) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  return impl_->FlushSpan(span);
}

void Logger::ForEachTag(fit::inline_function<void(const std::string&)> callback) const {
  if (!impl_) {
    return;
  }
  for (const std::string& tag : impl_->tags()) {
    callback(tag);
  }
}

}  // namespace fuchsia_logging

#endif
