// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_ETHERNET_DRIVERS_VIRTIO_VIRTIO_NET_DRIVER_H_
#define SRC_CONNECTIVITY_ETHERNET_DRIVERS_VIRTIO_VIRTIO_NET_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>

#include <memory>

#include "src/connectivity/ethernet/drivers/virtio/netdevice.h"

namespace virtio {

// The driver base implementation must be kept separate from the virtio Device implementation. The
// virtio Device base class can only be constructed with a specific set of parameters that we cannot
// provide in the fdf::DriverBase2 constructor.
class VirtioNetDriver : public fdf::DriverBase2 {
 public:
  VirtioNetDriver();

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  NetworkDevice* GetNetworkDevice() { return netdevice_.get(); }

 private:
  friend class NetworkDevice;

  // This is a virtual method so that it can be overridden in tests.
  virtual zx::result<std::unique_ptr<NetworkDevice>> CreateNetworkDevice(
      const std::shared_ptr<fdf::Namespace>& incoming, const std::optional<std::string>& node_name);

  std::unique_ptr<NetworkDevice> netdevice_;
};

}  // namespace virtio

#endif  // SRC_CONNECTIVITY_ETHERNET_DRIVERS_VIRTIO_VIRTIO_NET_DRIVER_H_
