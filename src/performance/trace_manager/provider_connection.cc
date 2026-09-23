// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "provider_connection.h"

#include <format>

namespace tracing {

ProviderConnection::ProviderConnection(
    fidl::ClientEnd<fuchsia_tracing_provider::ProviderV2> provider, uint32_t id, zx_koid_t pid,
    std::string name, async_dispatcher_t* dispatcher)
    : id(id), pid(pid), name(std::move(name)), provider(std::move(provider), dispatcher, this) {}

void ProviderConnection::on_fidl_error(fidl::UnbindInfo info) {
  if (on_unbound_) {
    on_unbound_(info);
  }
}

void ProviderConnection::RegisterForAlerts(fit::function<void(std::string_view alert)> cb) {
  alert_cb_ = std::move(cb);
}

void ProviderConnection::RegisterForBufferSave(
    fit::function<void(uint32_t wrapped_count, uint64_t durable_data_end)> buffer_save_cb) {
  buffer_save_cb_ = std::move(buffer_save_cb);
}

void ProviderConnection::OnSaveBuffer(
    fidl::Event<fuchsia_tracing_provider::ProviderV2::OnSaveBuffer>& event) {
  if (buffer_save_cb_) {
    buffer_save_cb_(event.wrapped_count(), event.durable_data_end());
  }
}

void ProviderConnection::OnAlert(
    fidl::Event<fuchsia_tracing_provider::ProviderV2::OnAlert>& event) {
  if (alert_cb_) {
    alert_cb_(event.name());
  }
}

void ProviderConnection::handle_unknown_event(
    fidl::UnknownEventMetadata<fuchsia_tracing_provider::ProviderV2> metadata) {
  // Ignore the unknown event.
}

std::string ProviderConnection::ToString() const {
  return std::format("#{} {{{}:{}}}", id, pid, name);
}

}  // namespace tracing
