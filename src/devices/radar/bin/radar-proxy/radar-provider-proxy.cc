// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "radar-provider-proxy.h"

#include <lib/syslog/cpp/macros.h>

#include <memory>

namespace radar {

std::unique_ptr<RadarProxy> RadarProxy::Create(async_dispatcher_t* dispatcher,
                                               RadarDeviceConnector* connector) {
  FX_LOGS(INFO) << "Burst reader proxying disabled";
  auto proxy = std::make_unique<radar::RadarProviderProxy>(dispatcher, connector);
  zx::result<> status = proxy->Init(dispatcher);
  if (status.is_error()) {
    FX_LOGS(ERROR) << "Failed to initialize RadarProxy: " << status.status_string();
    return nullptr;
  }
  return proxy;
}

RadarProviderProxy::~RadarProviderProxy() {
  // Close out any outstanding requests before destruction to avoid triggering an assert.
  for (auto& [server, completer] : connect_requests_) {
    completer.Close(ZX_ERR_PEER_CLOSED);
  }
}

void RadarProviderProxy::Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) {
  if (radar_client_) {
    ConnectClient(std::move(request.server()), completer.ToAsync());
  } else {
    // If there is no device connection, add this request to the queue to be completed later. This
    // avoids a race between discovering existing devices and the first connect request while we are
    // starting up.
    connect_requests_.emplace_back(std::move(request.server()), completer.ToAsync());
  }
}

zx::result<> RadarProviderProxy::AddProtocols(component::OutgoingDirectory* const outgoing) {
  zx::result result =
      outgoing->AddUnmanagedProtocol<fuchsia_hardware_radar::RadarBurstReaderProvider>(
          [&](fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> server_end) {
            fidl::BindServer(dispatcher_, std::move(server_end), this);
          });
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add RadarBurstReaderProvider protocol: " << result.status_string();
    return result;
  }

  return zx::ok();
}

void RadarProviderProxy::OnDeviceFound(
    fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end) {
  if (radar_client_) {
    return;
  }

  ConnectToRadarDevice(std::move(client_end));
}
void RadarProviderProxy::on_fidl_error(fidl::UnbindInfo info) {
  FX_PLOGS(ERROR, info.status()) << "Connection to radar device closed, attempting to reconnect";

  // Invalidate the client so that subsequent calls know that there is no connection to the driver.
  radar_client_ = {};

  // Check for available devices now, just in case one was added before the connection closed. If
  // not, the ServiceMemberWatcher will signal to connect when a new device becomes available.
  connector_->ConnectToFirstRadarDevice(
      [&](auto client_end) { return ConnectToRadarDevice(std::move(client_end)); });
}

bool RadarProviderProxy::ConnectToRadarDevice(
    fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end) {
  radar_client_.Bind(std::move(client_end), dispatcher_, this);

  // Now that we are connected to a device, complete any connect requests that are outstanding.
  for (auto& [server, completer] : connect_requests_) {
    ConnectClient(std::move(server), std::move(completer));
  }
  connect_requests_.clear();
  return true;
}

void RadarProviderProxy::ConnectClient(
    fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstReader> server,
    ConnectCompleter::Async completer) {
  radar_client_->Connect(std::move(server))
      .Then([async_completer = std::move(completer)](const auto& result) mutable {
        if (result.is_ok()) {
          async_completer.Reply(fit::ok());
        } else if (result.error_value().is_domain_error()) {
          async_completer.Reply(fit::error(result.error_value().domain_error()));
        } else if (result.error_value().is_framework_error()) {
          async_completer.Reply(fit::error(fuchsia_hardware_radar::StatusCode::kBindError));
        }
      });
}

}  // namespace radar
