// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/connectivity/ethernet/drivers/virtio/virtio_net_driver.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/virtio/driver_utils.h>

namespace {

constexpr char kDriverName[] = "virtio-net";

}  // namespace

namespace virtio {

VirtioNetDriver::VirtioNetDriver() : fdf::DriverBase2(kDriverName) {}

zx::result<> VirtioNetDriver::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result netdevice = CreateNetworkDevice(incoming, context.node_name());
  if (netdevice.is_error()) {
    fdf::error("Failed to create net device: {}", netdevice);
    return netdevice.take_error();
  }
  netdevice_ = std::move(netdevice.value());

  if (zx_status_t status = netdevice_->Init(); status != ZX_OK) {
    fdf::error("Failed to initialize net device: {}", zx_status_get_string(status));
    // Call Shutdown to clean up any device state. The driver framework will not call Stop
    // if Start fails so we need to perform this cleanup here.
    netdevice_->Shutdown();
    return zx::error(status);
  }

  return zx::ok();
}

void VirtioNetDriver::Stop(fdf::StopCompleter completer) {
  if (netdevice_) {
    netdevice_->Shutdown();
  }
  completer(zx::ok());
}

zx::result<std::unique_ptr<NetworkDevice>> VirtioNetDriver::CreateNetworkDevice(
    const std::shared_ptr<fdf::Namespace>& incoming, const std::optional<std::string>& node_name) {
  zx::result<fidl::ClientEnd<fuchsia_hardware_pci::Device>> pci =
      incoming->Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci.is_error()) {
    fdf::error("Failed to get pci client: {}", pci);
    return pci.take_error();
  }

  zx::result<std::pair<zx::bti, std::unique_ptr<virtio::Backend>>> bti_and_backend =
      virtio::GetBtiAndBackend(std::move(pci).value());
  if (!bti_and_backend.is_ok()) {
    fdf::error("GetBtiAndBackend failed: {}", bti_and_backend);
    return bti_and_backend.take_error();
  }
  auto [bti, backend] = std::move(bti_and_backend).value();

  return zx::ok(std::make_unique<NetworkDevice>(this, std::move(bti), std::move(backend), incoming,
                                                node_name));
}

}  // namespace virtio
