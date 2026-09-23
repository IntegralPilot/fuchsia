// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/virtio/backends/pci.h>
#include <lib/virtio/driver_utils_dfv1.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <memory>

#include <ddktl/device.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/zxlogf.h"

namespace virtio {

zx::result<std::pair<zx::bti, std::unique_ptr<virtio::Backend>>> GetBtiAndBackend(
    zx_device_t* bus_device) {
  zx::result client =
      ddk::Device<void>::DdkConnectFragmentFidlProtocol<fuchsia_hardware_pci::Service::Device>(
          bus_device, "pci");

  if (!client.is_ok()) {
    client = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
        bus_device);
  }

  if (!client.is_ok()) {
    zxlogf(ERROR, "virtio failed to find PciProtocol");
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  return GetBtiAndBackend(std::move(client.value()));
}

}  // namespace virtio
