// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_VIRTUALIZATION_BIN_VMM_DEVICE_VIRTIO_NET_SRC_CPP_GUEST_ETHERNET_CONTEXT_H_
#define SRC_VIRTUALIZATION_BIN_VMM_DEVICE_VIRTIO_NET_SRC_CPP_GUEST_ETHERNET_CONTEXT_H_

#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/result.h>

#include <memory>

#include "src/connectivity/network/drivers/network-device/device/public/network_device.h"

class GuestEthernetContext {
 public:
  static zx::result<std::unique_ptr<GuestEthernetContext>> Create();
  ~GuestEthernetContext();

  fdf::Dispatcher* SyncDispatcher() { return &sync_dispatcher_; }
  network::DeviceInterfaceDispatchers Dispatchers() { return dispatchers_->Unowned(); }

 private:
  GuestEthernetContext() = default;

  fdf::Dispatcher sync_dispatcher_;
  libsync::Completion sync_dispatcher_shutdown_;

  std::unique_ptr<network::OwnedDeviceInterfaceDispatchers> dispatchers_;
};

#endif  // SRC_VIRTUALIZATION_BIN_VMM_DEVICE_VIRTIO_NET_SRC_CPP_GUEST_ETHERNET_CONTEXT_H_
