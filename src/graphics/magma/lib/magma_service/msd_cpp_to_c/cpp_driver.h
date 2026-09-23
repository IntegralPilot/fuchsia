// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_DRIVER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_DRIVER_H_

#include <lib/magma_service/msd.h>
#include <lib/magma_service/msd_c.h>

#include <memory>

namespace msd {

struct DeviceHandle {
  struct MsdPlatformDevice* platform_device;
};

class CppDriver : public msd::Driver {
 public:
  CppDriver();

  // msd::Driver implementation.
  std::unique_ptr<msd::Device> MsdCreateDevice(msd::DeviceHandle* device_handle) override;

  std::unique_ptr<msd::Buffer> MsdImportBuffer(zx::vmo vmo, uint64_t client_id) override;

  magma_status_t MsdImportSemaphore(zx::handle handle, uint64_t client_id, uint64_t flags,
                                    std::unique_ptr<msd::Semaphore>* out) override;
};
}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_DRIVER_H_
