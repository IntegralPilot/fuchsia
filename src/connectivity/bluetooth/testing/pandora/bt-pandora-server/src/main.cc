// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include "pandora_fidl_server.h"
#include "pandora_grpc_server.h"

int main(int argc, char* argv[]) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  PandoraGrpcServer pandora_server(dispatcher);

  result = outgoing.AddUnmanagedProtocol<fuchsia_bluetooth_pandora::GrpcServerController>(
      [dispatcher, &pandora_server](
          fidl::ServerEnd<fuchsia_bluetooth_pandora::GrpcServerController> server_end) {
        FX_LOGS(INFO)
            << "Incoming connection for "
            << fidl::DiscoverableProtocolName<fuchsia_bluetooth_pandora::GrpcServerController>;
        PandoraImpl::BindSelfManagedServer(dispatcher, std::move(server_end), pandora_server);
      });
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Pandora protocol: " << result.status_string();
    return -1;
  }

  FX_LOGS(INFO) << "Pandora FIDL server starting";
  loop.Run();
  FX_LOGS(INFO) << "Pandora FIDL server stopped";

  return 0;
}
