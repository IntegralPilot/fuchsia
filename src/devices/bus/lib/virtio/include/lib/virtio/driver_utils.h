// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_DRIVER_UTILS_H_
#define SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_DRIVER_UTILS_H_

#include <fidl/fuchsia.hardware.pci/cpp/fidl.h>
#include <lib/virtio/device.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

namespace virtio {
zx::result<std::pair<zx::bti, std::unique_ptr<virtio::Backend>>> GetBtiAndBackend(
    fidl::ClientEnd<fuchsia_hardware_pci::Device> pci);
}  // namespace virtio

#endif  // SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_DRIVER_UTILS_H_
