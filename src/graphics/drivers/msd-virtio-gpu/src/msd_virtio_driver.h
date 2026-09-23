// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_VIRTIO_DRIVER_H
#define MSD_VIRTIO_DRIVER_H

#include <lib/magma_service/msd.h>

#include <memory>

class MsdVirtioDriver : public msd::Driver {
 public:
  // msd::Driver implementation.
  std::unique_ptr<msd::Device> MsdCreateDevice(msd::DeviceHandle* device_handle) override;
};

#endif  // MSD_VIRTIO_DRIVER_H
