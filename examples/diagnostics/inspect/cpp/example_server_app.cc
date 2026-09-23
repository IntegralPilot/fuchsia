// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "example_server_app.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

namespace example {

ExampleServerApp::ExampleServerApp() {
  // [START initialization]
  inspector_ = std::make_unique<inspect::ComponentInspector>(async_get_default_dispatcher(),
                                                             inspect::PublishOptions{});
  // [END initialization]

  // [START properties]
  // Attach properties to the root node of the tree
  inspect::Node& root_node = inspector_->root();
  // Important: Hold references to properties and don't let them go out of scope.
  auto total_requests = root_node.CreateUint("total_requests", 0);
  auto bytes_processed = root_node.CreateUint("bytes_processed", 0);
  // [END properties]

  // [START health_check]
  inspector_->Health().StartingUp();

  // [START_EXCLUDE]
  echo_stats_ = std::make_shared<EchoConnectionStats>(EchoConnectionStats{
      std::move(bytes_processed),
      std::move(total_requests),
  });

  outgoing_directory_.emplace(component::OutgoingDirectory(async_get_default_dispatcher()));
  zx::result result = outgoing_directory_->AddUnmanagedProtocol<fidl_examples_routing_echo::Echo>(
      [this](fidl::ServerEnd<fidl_examples_routing_echo::Echo> server_end) {
        fidl::BindServer(async_get_default_dispatcher(), std::move(server_end),
                         std::make_unique<EchoConnection>(echo_stats_));
      });

  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Echo service: " << result.status_string();
  }

  zx::result serve_result = outgoing_directory_->ServeFromStartupInfo();
  if (serve_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << serve_result.status_string();
  }
  // [END_EXCLUDE]

  inspector_->Health().Ok();
  // [END health_check]
}

}  // namespace example
