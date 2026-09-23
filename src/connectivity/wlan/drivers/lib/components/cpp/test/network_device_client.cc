// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "network_device_client.h"

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/protocol.h>
#include <lib/fdio/directory.h>

namespace netdriver = fuchsia_hardware_network_driver;

namespace wlan::drivers::components::test {

NetworkDeviceClient::NetworkDeviceClient() = default;
NetworkDeviceClient::NetworkDeviceClient(NetworkDeviceClient&&) noexcept = default;
NetworkDeviceClient& NetworkDeviceClient::operator=(NetworkDeviceClient&&) noexcept = default;

zx::result<NetworkDeviceClient> NetworkDeviceClient::Create(
    fidl::UnownedClientEnd<fuchsia_io::Directory> driver_outgoing, fdf_dispatcher_t* dispatcher) {
  if (dispatcher == nullptr) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  NetworkDeviceClient client;
  if (zx_status_t status = client.Connect(driver_outgoing, dispatcher); status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(client));
}

zx_status_t NetworkDeviceClient::Connect(
    fidl::UnownedClientEnd<fuchsia_io::Directory> driver_outgoing, fdf_dispatcher_t* dispatcher) {
  if (dispatcher == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto endpoints = fdf::CreateEndpoints<netdriver::Service::NetworkDeviceImpl::ProtocolType>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create netdev endpoints: {}", endpoints.status_string());
    return endpoints.status_value();
  }

  zx::channel client_token, server_token;
  if (zx_status_t status = zx::channel::create(0, &client_token, &server_token); status != ZX_OK) {
    fdf::error("Failed to create channel: {}", zx_status_get_string(status));
    return status;
  }
  if (zx_status_t status = fdf::ProtocolConnect(
          std::move(client_token), fdf::Channel(endpoints->server.TakeChannel().release()));
      status != ZX_OK) {
    fdf::error("Failed to connect protocol: {}", zx_status_get_string(status));
    return status;
  }

  std::string path = component::kServiceDirectoryTrailingSlash +
                     component::MakeServiceMemberPath<netdriver::Service::NetworkDeviceImpl>(
                         component::kDefaultInstance);

  if (zx_status_t status = fdio_service_connect_at(driver_outgoing.channel()->get(), path.c_str(),
                                                   server_token.release());
      status != ZX_OK) {
    fdf::error("Failed to connect to service: {}", zx_status_get_string(status));
    return status;
  }

  client_.Bind(std::move(endpoints->client), dispatcher);
  return ZX_OK;
}

}  // namespace wlan::drivers::components::test
