// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include "src/connectivity/network/testing/netemul/network-context/lib/network_context.h"

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"network-context"}).BuildAndInitialize();
  FX_LOGS(INFO) << "starting...";

  std::unique_ptr context = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  context->outgoing()->AddPublicService(
      fidl::InterfaceRequestHandler<fuchsia::netemul::network::NetworkContext>(
          [&context](fidl::InterfaceRequest<fuchsia::netemul::network::NetworkContext> request) {
            auto impl = std::make_unique<netemul::NetworkContext>();
            impl->SetNetworkTunHandler(
                [&context](fidl::InterfaceRequest<fuchsia::net::tun::Control> req) {
                  zx_status_t status =
                      context->svc()->Connect<fuchsia::net::tun::Control>(std::move(req));
                  if (status != ZX_OK) {
                    FX_PLOGS(ERROR, status)
                        << "failed to connect request to " << fuchsia::net::tun::Control::Name_;
                  }
                });
            impl->GetHandler()(std::move(request));
            // Intentionally leak the `std::unique_ptr` to let the
            // `NetworkContext` clean itself up when all clients connections to
            // it are closed.
            [[maybe_unused]] auto ptr = impl.release();
          }));
  return loop.Run();
}
