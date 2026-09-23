// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pandora_fidl_server.h"

#include <lib/syslog/cpp/macros.h>

#include "fidl/fuchsia.bluetooth.pandora/cpp/common_types.h"

using fuchsia_bluetooth_pandora::ServiceError;

PandoraImpl::PandoraImpl(PandoraGrpcServer& grpc_server) : grpc_server_(grpc_server) {}

void PandoraImpl::Start(StartRequest& request, StartCompleter::Sync& completer) {
  if (!request.port()) {
    FX_LOGS(ERROR) << "Pandora FIDL Start request empty port";
    completer.Reply(fit::error(ServiceError::kFailed));
    return;
  }

  switch (grpc_server_.Run(*request.port())) {
    case ZX_OK: {
      completer.Reply(fit::ok());
      break;
    }

    case ZX_ERR_ALREADY_EXISTS: {
      completer.Reply(fit::error(ServiceError::kAlreadyRunning));
      break;
    }

    case ZX_ERR_INTERNAL: {
      completer.Reply(fit::error(ServiceError::kFailed));
      break;
    }

    default: {
      FX_LOGS(WARNING) << "Failed to start Pandora server";
      completer.Reply(fit::error(ServiceError::kFailed));
      break;
    }
  }
}

void PandoraImpl::Stop(StopCompleter::Sync& completer) {
  grpc_server_.Shutdown();
  completer.Reply();
}

void PandoraImpl::BindSelfManagedServer(
    async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_bluetooth_pandora::GrpcServerController> server_end,
    PandoraGrpcServer& grpc_server) {
  std::unique_ptr impl = std::make_unique<PandoraImpl>(grpc_server);
  PandoraImpl* impl_ptr = impl.get();

  fidl::ServerBindingRef binding_ref = fidl::BindServer(
      dispatcher, std::move(server_end), std::move(impl), std::mem_fn(&PandoraImpl::OnUnbound));
  impl_ptr->binding_ref_.emplace(std::move(binding_ref));
}

void PandoraImpl::OnUnbound(
    fidl::UnbindInfo info,
    fidl::ServerEnd<fuchsia_bluetooth_pandora::GrpcServerController> server_end) {
  if (info.is_user_initiated()) {
    FX_LOGS(INFO) << "Unbinding Pandora FIDL server";
    return;
  }
  if (info.is_peer_closed()) {
    FX_LOGS(INFO) << "Client disconnected from Pandora FIDL server";
  } else {
    // Treat other unbind causes as errors.
    FX_LOGS(INFO) << "Pandora FIDL server error: " << info;
  }
}

void PandoraImpl::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_bluetooth_pandora::GrpcServerController> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Received an unknown method with ordinal " << metadata.method_ordinal;
}
