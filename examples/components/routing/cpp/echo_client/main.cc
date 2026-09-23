// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START imports]
#include <fidl/fidl.examples.routing.echo/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/string.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdlib>
#include <iostream>
#include <string>
// [END imports]

// [START main_body]
int main(int argc, const char* argv[], char* envp[]) {
  // [START_EXCLUDE silent]
  // TODO(https://fxbug.dev/42179369): Consider migrating to async FIDL API
  // [END_EXCLUDE]
  // Set tags for logging.
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"echo_client"}).BuildAndInitialize();

  // Connect to FIDL protocol
  zx::result client_end = component::Connect<fidl_examples_routing_echo::Echo>();
  if (client_end.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to Echo protocol: " << client_end.status_string();
    return EXIT_FAILURE;
  }
  fidl::SyncClient client(std::move(client_end.value()));

  // Send messages over FIDL interface for each argument
  fidl::StringPtr response = nullptr;
  for (int i = 1; i < argc; i++) {
    fidl::Result response = client->EchoString({argv[i]});

    if (response.is_error()) {
      FX_LOGS(ERROR) << "echo_string failed: " << response.error_value();
      return EXIT_FAILURE;
    }
    if (!response->response().has_value()) {
      FX_LOGS(ERROR) << "echo_string got empty result";
      return EXIT_FAILURE;
    }
    const std::string& response_value = response->response().value();
    FX_LOG_KV(INFO, "Server response", FX_KV("response", response_value));
  }

  return 0;
}
// [END main_body]
