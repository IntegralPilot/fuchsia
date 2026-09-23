// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_PROVIDER_CONNECTION_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_PROVIDER_CONNECTION_H_

#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>

#include <format>

namespace tracing {

class ProviderConnection : public fidl::AsyncEventHandler<fuchsia_tracing_provider::ProviderV2> {
 public:
  ProviderConnection(fidl::ClientEnd<fuchsia_tracing_provider::ProviderV2> provider, uint32_t id,
                     zx_koid_t pid, std::string name, async_dispatcher_t* dispatcher);
  ~ProviderConnection() override = default;

  ProviderConnection(const ProviderConnection& value) = delete;
  ProviderConnection& operator=(const ProviderConnection&) = delete;

  ProviderConnection(ProviderConnection&& value) = default;
  ProviderConnection& operator=(ProviderConnection&&) = default;

  std::string ToString() const;

  template <class It>
  It format_to(It out) const {
    return std::format_to(out, "#{} ({}:{})", id, pid, name);
  }

  void RegisterForAlerts(fit::function<void(std::string_view alert)> cb);
  void RegisterForBufferSave(
      fit::function<void(uint32_t wrapped_count, uint64_t durable_data_end)> buffer_save_cb);

  uint32_t id;
  zx_koid_t pid;
  std::string name;
  fidl::Client<fuchsia_tracing_provider::ProviderV2> provider;

  void SetOnUnbound(fit::function<void(fidl::UnbindInfo)> on_unbound) {
    on_unbound_ = std::move(on_unbound);
  }

  /// A buffer is full and needs to be saved (streaming mode only).
  void OnSaveBuffer(
      fidl::Event<fuchsia_tracing_provider::ProviderV2::OnSaveBuffer>& event) override;

  /// Sends an alert.
  void OnAlert(fidl::Event<fuchsia_tracing_provider::ProviderV2::OnAlert>& event) override;

  void on_fidl_error(fidl::UnbindInfo info) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_tracing_provider::ProviderV2> metadata) override;

 private:
  fit::function<void(fidl::UnbindInfo)> on_unbound_;
  fit::function<void(std::string_view alert)> alert_cb_;
  fit::function<void(uint32_t wrapped_count, uint64_t durable_data_end)> buffer_save_cb_;
};

struct ProviderSpec {
  std::optional<uint32_t> buffer_size_megabytes;
  std::vector<std::string> categories;
};

using ProviderSpecMap = std::map<std::string, ProviderSpec>;

}  // namespace tracing

template <>
struct std::formatter<tracing::ProviderConnection> {
  constexpr auto parse(const std::format_parse_context& ctx) {
    auto it = ctx.begin();
    assert(it == ctx.end() || *it == '}');
    return it;
  }
  template <class FormatContext>
  auto format(const tracing::ProviderConnection& rhs, FormatContext& ctx) const {
    return rhs.format_to(ctx.out());
  }
};
#endif  // SRC_PERFORMANCE_TRACE_MANAGER_PROVIDER_CONNECTION_H_
