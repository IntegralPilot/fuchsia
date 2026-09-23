// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_NETWORK_DEVICE_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_NETWORK_DEVICE_H_

#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>

#include <memory>

#include "device/public/network_device.h"

namespace network {

class NetworkDevice;

// Creates `fuchsia_hardware_network_driver::NetworkDeviceImpl` endpoints for a
// parent device that is backed by the FIDL based driver runtime.
class FidlNetworkDeviceImplBinder : public NetworkDeviceImplBinder {
 public:
  explicit FidlNetworkDeviceImplBinder(std::shared_ptr<fdf::Namespace> incoming)
      : incoming_(std::move(incoming)) {}

  zx::result<fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl>> Bind() override;

 private:
  std::shared_ptr<fdf::Namespace> incoming_;
};

class NetworkDevice : public fdf::DriverBase2 {
 public:
  NetworkDevice();
  ~NetworkDevice() override;

  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;
  void Stop(fdf::StopCompleter completer) override;

  NetworkDeviceInterface* GetInterface() { return device_.get(); }

 private:
  void Connect(fidl::ServerEnd<fuchsia_hardware_network::Device> request);
  zx::result<std::unique_ptr<NetworkDeviceImplBinder>> CreateImplBinder(
      const std::shared_ptr<fdf::Namespace>& incoming);

  std::unique_ptr<OwnedDeviceInterfaceDispatchers> dispatchers_;

  std::unique_ptr<NetworkDeviceInterface> device_;

  // These are used for the child node created to enable discovery through devfs. The child node has
  // to be kept alive for the child to remain alive and discoverable through devfs.
  fidl::WireSyncClient<fuchsia_driver_framework::Node> child_node_;
  driver_devfs::Connector<fuchsia_hardware_network::Device> devfs_connector_{
      fit::bind_member<&NetworkDevice::Connect>(this)};
};

}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_NETWORK_DEVICE_H_
