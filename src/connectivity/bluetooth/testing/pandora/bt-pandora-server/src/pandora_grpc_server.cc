// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pandora_grpc_server.h"

#include <lib/syslog/cpp/macros.h>

#include <string>

#include "src/connectivity/bluetooth/testing/bt-affordances/ffi_c/bindings.h"

PandoraGrpcServer::PandoraGrpcServer(async_dispatcher_t* dispatcher)
    : a2dp_service_(dispatcher),
      host_service_(dispatcher),
      l2cap_service_(dispatcher),
      security_service_(dispatcher),
      security_storage_service_(dispatcher)
#ifdef PANDORA_WITH_GATT
      ,
      gatt_service_(dispatcher)
#endif
{
}

PandoraGrpcServer::~PandoraGrpcServer() { Shutdown(); }

zx_status_t PandoraGrpcServer::Run(uint16_t port, bool verbose) {
  if (IsRunning()) {
    return ZX_ERR_ALREADY_EXISTS;
  }

  if (verbose) {
    setenv("GRPC_TRACE", "http,api", 1);
    setenv("GRPC_VERBOSITY", "info", 1);
  }

  grpc::ServerBuilder builder;
  const std::string address = "0.0.0.0:" + std::to_string(port);
  builder.AddListeningPort(address, grpc::InsecureServerCredentials());
  builder.RegisterService(&a2dp_service_);
  builder.RegisterService(&host_service_);
  builder.RegisterService(&l2cap_service_);
  builder.RegisterService(&security_service_);
  builder.RegisterService(&security_storage_service_);
#ifdef PANDORA_WITH_GATT
  builder.RegisterService(&gatt_service_);
#endif  // PANDORA_WITH_GATT

  FX_LOGS(INFO) << "Server listening on " << address;
  server_ = builder.BuildAndStart();
  return (server_ ? ZX_OK : ZX_ERR_INTERNAL);
}

void PandoraGrpcServer::Shutdown() {
  if (IsRunning()) {
    FX_LOGS(INFO) << "Shutting down Pandora server";
    server_->Shutdown();
    server_.reset(nullptr);

    zx_status_t status = stop_rust_affordances();
    if (status == ZX_ERR_BAD_STATE) {
      FX_LOGS(ERROR) << "Tried to stop Rust affordances but they weren't running";
    } else if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Encountered error stopping Rust affordances (check logs)";
    }
  }
}

bool PandoraGrpcServer::IsRunning() { return static_cast<bool>(server_); }
