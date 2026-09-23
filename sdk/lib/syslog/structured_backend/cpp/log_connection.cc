// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/structured_backend/cpp/log_connection.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(29)

namespace fuchsia_logging::internal {

zx::result<std::pair<LogConnection, std::optional<FuchsiaLogSeverity>>> LogConnection::Create(
    fidl::UnownedClientEnd<fuchsia_logger::LogSink> client_end) {
  class EventHandler : public fidl::WireSyncEventHandler<fuchsia_logger::LogSink> {
   public:
    zx::iob& iob() { return iob_; }
    std::optional<FuchsiaLogSeverity> min_severity() const { return min_severity_; }

    void OnInit(fidl::WireEvent<fuchsia_logger::LogSink::OnInit>* event) override {
      if (event->has_buffer()) {
        iob_ = std::move(event->buffer());
      }
      if (event->has_interest()) {
        const auto& interest = event->interest();
        if (interest.has_min_severity()) {
          min_severity_ = static_cast<FuchsiaLogSeverity>(interest.min_severity());
        }
      }
    }

    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_logger::LogSink> metadata) override {}

   private:
    zx::iob iob_;
    std::optional<FuchsiaLogSeverity> min_severity_;
  } handler;

  if (fidl::Status status = handler.HandleOneEvent(client_end); !status.ok()) {
    return zx::error(status.status());
  }

  if (!handler.iob().is_valid()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(std::make_pair(LogConnection(std::move(handler.iob())), handler.min_severity()));
}

zx::result<> LogConnection::FlushSpan(cpp20::span<const uint8_t> data) const {
  if (data.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  zx_iovec_t vector[] = {{.buffer = const_cast<uint8_t*>(data.data()), .capacity = data.size()}};
  return iob_.writev({}, 0, vector, std::size(vector));
}

}  // namespace fuchsia_logging::internal

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(29)
