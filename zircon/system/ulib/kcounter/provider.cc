// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.inspect/cpp/fidl.h>
#include <fidl/fuchsia.inspect/cpp/wire.h>
#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/inspect/component/cpp/service.h>
#include <lib/kcounter/provider.h>
#include <lib/zx/channel.h>
#include <string.h>
#include <zircon/status.h>

#include "kcounter.h"

namespace {

/// Counter exposes kernel counter data in two ways:
///   fuchsia.kernel.Counter:
///     This protocol allows direct access to the Inspect VMO managed in the kernel
///
///   fuchsia.inspect.Tree:
///     This protocol is the standard Inspect tree server. It is how the kernel counter data
///     is exposed to tools such as `ffx inspect`.
class Counter : public fidl::WireServer<fuchsia_kernel::Counter>,
                public fidl::WireServer<fuchsia_inspect::Tree> {
 public:
  Counter(async_dispatcher_t* dispatcher, kcounter::VmoToInspectMapper mapper)
      : dispatcher_(dispatcher), mapper_(std::move(mapper)) {}

 private:
  void GetInspectVmo(GetInspectVmoCompleter::Sync& completer) override {
    fuchsia_mem::wire::Buffer buffer;
    if (zx_status_t status = mapper_.GetInspectVMO(&buffer.vmo); status != ZX_OK) {
      return completer.Reply(status, {});
    }
    if (zx_status_t status = buffer.vmo.get_size(&buffer.size); status != ZX_OK) {
      return completer.Reply(status, {});
    }
    completer.Reply(ZX_OK, std::move(buffer));
  }

  void UpdateInspectVmo(UpdateInspectVmoCompleter::Sync& completer) override {
    completer.Reply(mapper_.UpdateInspectVMO());
  }

  /// Get the VMO handle for the Inspector handled by this server.
  /// `UpdateInspectVMO` is executed on each call.
  void GetContent(GetContentCompleter::Sync& completer) override {
    mapper_.UpdateInspectVMO();

    fidl::Arena arena;
    auto content_builder = fuchsia_inspect::wire::TreeContent::Builder(arena);
    fuchsia_mem::wire::Buffer buffer;

    // send live VMO as the only writes that happen to this Inspector happen in `UpdateInspectVMO`
    mapper_.GetInspectVMO(&buffer.vmo);

    content_builder.buffer(std::move(buffer));
    completer.Reply(content_builder.Build());
  }

  /// Not implemented. Will never be executed.
  void OpenChild(OpenChildRequestView request, OpenChildCompleter::Sync& completer) override {
    // This is intentionally left blank as `Counter::ListChildNames` returns a server
    // handle with an empty name list. That precludes this function ever being executed.
    //
    // If lazy nodes are ever added to the wrapped Inspector, this and `ListChildNames` will either
    // need a full implementation, or the method for exposing the Inspector will need to change.
  }

  /// Returns a handle to a server that will always return 0 names.
  void ListChildNames(ListChildNamesRequestView request,
                      ListChildNamesCompleter::Sync& completer) override {
    // Always return 0 names
    inspect::TreeNameIterator::StartSelfManagedServer(
        dispatcher_, std::move(request->tree_iterator), std::vector<std::string>{});
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_inspect::Tree> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_UNAVAILABLE);
  }

  async_dispatcher_t* dispatcher_;
  kcounter::VmoToInspectMapper mapper_;
};

zx_status_t Connect(void* ctx, async_dispatcher_t* dispatcher, const char* service_name,
                    zx_handle_t request) {
  zx::channel channel{request};
  if (fidl::DiscoverableProtocolName<fuchsia_kernel::Counter> == service_name) {
    fidl::BindServer(dispatcher, fidl::ServerEnd<fuchsia_kernel::Counter>{std::move(channel)},
                     static_cast<Counter*>(ctx));
    return ZX_OK;
  }

  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t Init(void** ctx) {
  auto* dispatcher = static_cast<async_dispatcher_t*>(*ctx);

  auto* counter = new Counter(dispatcher, kcounter::VmoToInspectMapper{});

  auto endpoints = fidl::CreateEndpoints<fuchsia_inspect::Tree>();
  fidl::BindServer(dispatcher, std::move(endpoints->server), counter);

  auto inspect_sink_client = component::Connect<fuchsia_inspect::InspectSink>().value();
  fidl::Client client(std::move(inspect_sink_client), dispatcher);
  auto result = client->Publish({{.tree = std::move(endpoints->client)}});
  ZX_ASSERT(result.is_ok());

  *ctx = counter;
  return ZX_OK;
}

void Release(void* ctx) { delete static_cast<Counter*>(ctx); }

constexpr const char* kKcounterServices[] = {
    fidl::DiscoverableProtocolName<fuchsia_kernel::Counter>,
    nullptr,
};

constexpr zx_service_ops_t kKcounterOps = {
    .init = Init,
    .connect = Connect,
    .release = Release,
};

constexpr zx_service_provider_t kcounter_service_provider = {
    .version = SERVICE_PROVIDER_VERSION,
    .services = kKcounterServices,
    .ops = &kKcounterOps,
};

}  // namespace

const zx_service_provider_t* kcounter_get_service_provider() { return &kcounter_service_provider; }
