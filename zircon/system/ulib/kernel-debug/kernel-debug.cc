// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/kernel-debug/kernel-debug.h>
#include <lib/zircon-internal/ktrace.h>
#include <lib/zx/resource.h>

namespace {

class DebugBroker : public fidl::WireServer<fuchsia_kernel::DebugBroker> {
 public:
  explicit DebugBroker(zx::unowned_resource tracing_resource)
      : tracing_resource_(std::move(tracing_resource)) {}

 private:
  void SendDebugCommand(SendDebugCommandRequestView request,
                        SendDebugCommandCompleter::Sync& completer) override {
    completer.Reply(zx_debug_send_command(tracing_resource_->get(), request->command.data(),
                                          request->command.size()));
  }

  void SetTracingEnabled(SetTracingEnabledRequestView request,
                         SetTracingEnabledCompleter::Sync& completer) override {
    zx_status_t status;
    if (request->enabled) {
      status =
          zx_ktrace_control(tracing_resource_->get(), KTRACE_ACTION_START, KTRACE_GRP_ALL, nullptr);
    } else {
      status = zx_ktrace_control(tracing_resource_->get(), KTRACE_ACTION_STOP, 0, nullptr);
      if (status == ZX_OK) {
        status = zx_ktrace_control(tracing_resource_->get(), KTRACE_ACTION_REWIND, 0, nullptr);
      }
    }
    completer.Reply(status);
  }

  const zx::unowned_resource tracing_resource_;
};

zx_status_t Connect(void* ctx, async_dispatcher_t* dispatcher, const char* service_name,
                    zx_handle_t request) {
  zx::channel channel{request};
  if (fidl::DiscoverableProtocolName<fuchsia_kernel::DebugBroker> == service_name) {
    const zx_handle_t resource = static_cast<zx_handle_t>(reinterpret_cast<uintptr_t>(ctx));
    fidl::BindServer(dispatcher, fidl::ServerEnd<fuchsia_kernel::DebugBroker>{std::move(channel)},
                     std::make_unique<DebugBroker>(zx::unowned_resource{resource}));
    return ZX_OK;
  }

  return ZX_ERR_NOT_SUPPORTED;
}

constexpr const char* kServices[] = {
    fidl::DiscoverableProtocolName<fuchsia_kernel::DebugBroker>,
    nullptr,
};

constexpr zx_service_ops_t kServiceOps = {
    .init = nullptr,
    .connect = Connect,
    .release = nullptr,
};

constexpr zx_service_provider_t kDebugBrokerServiceProvider = {
    .version = SERVICE_PROVIDER_VERSION,
    .services = kServices,
    .ops = &kServiceOps,
};

}  // namespace

const zx_service_provider_t* kernel_debug_get_service_provider() {
  return &kDebugBrokerServiceProvider;
}
