// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.resolution/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/fidl/cpp/wire/server.h>

#include <memory>

using Resolver = fuchsia_component_resolution::Resolver;

class ResolverImpl final : public fidl::WireServer<Resolver> {
 public:
  ResolverImpl(fidl::ClientEnd<fuchsia_component_resolution::Resolver> client,
               async_dispatcher_t* dispatcher)
      : endpoint_(std::move(client), dispatcher) {}

  void Resolve(ResolveRequestView request, ResolveCompleter::Sync& completer) override {
    endpoint_->Resolve(request->component_url)
        .Then([completer = completer.ToAsync()](
                  fidl::WireUnownedResult<Resolver::Resolve>& result) mutable {
          if (!result.ok()) {
            completer.Close(result.status());
            return;
          }
          completer.Reply(result.value());
        });
  }

  void ResolveWithContext(ResolveWithContextRequestView request,
                          ResolveWithContextCompleter::Sync& completer) override {
    endpoint_->ResolveWithContext(request->component_url, request->context)
        .Then([completer = completer.ToAsync()](
                  fidl::WireUnownedResult<Resolver::ResolveWithContext>& result) mutable {
          if (!result.ok()) {
            completer.Close(result.status());
            return;
          }
          completer.Reply(result.value());
        });
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<Resolver> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  fidl::WireClient<Resolver> endpoint_;
};

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  component::OutgoingDirectory outgoing_directory = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing_directory.ServeFromStartupInfo();
  if (result.is_error()) {
    return 1;
  }

  result = outgoing_directory.AddUnmanagedProtocol<fuchsia_component_resolution::Resolver>(
      [dispatcher](fidl::ServerEnd<fuchsia_component_resolution::Resolver> server_end) {
        auto client = component::Connect<fuchsia_component_resolution::Resolver>();
        if (client.is_error()) {
          return;
        }
        fidl::BindServer(dispatcher, std::move(server_end),
                         std::make_unique<ResolverImpl>(std::move(client.value()), dispatcher));
      });

  return loop.Run();
}
