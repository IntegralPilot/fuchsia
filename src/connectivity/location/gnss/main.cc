// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include "gnss_service.h"

int main(void) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithDispatcher(dispatcher).WithTags({"gnss_service"}).BuildAndInitialize();
  gnss::GnssService service(dispatcher);
  component::OutgoingDirectory outgoing(dispatcher);
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  // Add the protocol handler. The lambda now captures the *single* service instance.
  result = outgoing.AddUnmanagedProtocol<fuchsia_location_gnss::Provider>(
      // Capture the service instance by reference.
      // Capture the dispatcher by value (it's just a pointer).
      [&service, dispatcher](fidl::ServerEnd<fuchsia_location_gnss::Provider> server_end) {
        FX_LOGS(INFO) << "Incoming connection for "
                      << fidl::DiscoverableProtocolName<fuchsia_location_gnss::Provider>;
        // Add the new client connection to the *existing* service's binding group.
        service.AddBinding(dispatcher, std::move(server_end));
      });

  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Provider protocol: " << result.status_string();
    return -1;
  }

  FX_LOGS(INFO) << "Running Gnss service.";

  loop.Run();
}
