// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "msd_virtio_driver.h"

#include <lib/magma/util/macros.h>

#include "msd_virtio_device.h"

std::unique_ptr<msd::Device> MsdVirtioDriver::MsdCreateDevice(msd::DeviceHandle* device_handle) {
  return std::make_unique<MsdVirtioDevice>(static_cast<VirtioGpuControl*>(device_handle));
}

// static
std::unique_ptr<msd::Driver> msd::Driver::MsdCreate() {
  return std::make_unique<MsdVirtioDriver>();
}
