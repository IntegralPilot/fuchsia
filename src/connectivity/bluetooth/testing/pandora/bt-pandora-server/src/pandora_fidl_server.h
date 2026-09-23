// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_PANDORA_FIDL_SERVER_H_
#define SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_PANDORA_FIDL_SERVER_H_

#include <fidl/fuchsia.bluetooth.pandora/cpp/fidl.h>

#include "pandora_grpc_server.h"

class PandoraImpl : public fidl::Server<fuchsia_bluetooth_pandora::GrpcServerController> {
 public:
  explicit PandoraImpl(PandoraGrpcServer& grpc_server);

  // Launch Pandora gRPC server.
  void Start(StartRequest& request, StartCompleter::Sync& completer) override;

  // Stop Pandora gRPC server if running.
  void Stop(StopCompleter::Sync& completer) override;

  static void BindSelfManagedServer(
      async_dispatcher_t* dispatcher,
      fidl::ServerEnd<fuchsia_bluetooth_pandora::GrpcServerController> server_end,
      PandoraGrpcServer& grpc_server);

  void OnUnbound(fidl::UnbindInfo info,
                 fidl::ServerEnd<fuchsia_bluetooth_pandora::GrpcServerController> server_end);

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_bluetooth_pandora::GrpcServerController> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  PandoraGrpcServer& grpc_server_;
  std::optional<fidl::ServerBindingRef<fuchsia_bluetooth_pandora::GrpcServerController>>
      binding_ref_;
};

#endif  // SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_PANDORA_FIDL_SERVER_H_
