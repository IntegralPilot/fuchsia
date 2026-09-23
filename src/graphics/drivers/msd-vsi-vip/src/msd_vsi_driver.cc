// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "msd_vsi_driver.h"

#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

#include "msd_vsi_device.h"
#include "src/graphics/drivers/msd-vsi-vip/src/msd_vsi_semaphore.h"

void MsdVsiDriver::Destroy(MsdVsiDriver* drv) { delete drv; }

std::unique_ptr<msd::Device> MsdVsiDriver::MsdCreateDevice(msd::DeviceHandle* device_handle) {
  constexpr bool kEnableSuspend = true;
  std::unique_ptr<MsdVsiDevice> device = MsdVsiDevice::Create(device_handle, kEnableSuspend);
  if (!device) {
    return DRETP(nullptr, "Failed to create device");
  }

  bool start_device_thread = (configure_flags() & MSD_DRIVER_CONFIG_TEST_NO_DEVICE_THREAD) == 0;
  if (start_device_thread) {
    device->StartDeviceThread();
  }

  return device;
}

std::unique_ptr<msd::Buffer> MsdVsiDriver::MsdImportBuffer(zx::vmo vmo, uint64_t client_id) {
  auto buffer = MsdVsiBuffer::Import(std::move(vmo), client_id);

  if (!buffer) {
    return DRETP(nullptr, "Failed to import buffer");
  }

  return std::make_unique<MsdVsiAbiBuffer>(std::move(buffer));
}

magma_status_t MsdVsiDriver::MsdImportSemaphore(zx::handle handle, uint64_t client_id,
                                                uint64_t flags,
                                                std::unique_ptr<msd::Semaphore>* semaphore_out) {
  auto semaphore = magma::PlatformSemaphore::Import(std::move(handle), flags);
  if (!semaphore)
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "couldn't import semaphore handle");

  semaphore->set_local_id(client_id);

  *semaphore_out = std::make_unique<MsdVsiAbiSemaphore>(
      std::shared_ptr<magma::PlatformSemaphore>(std::move(semaphore)));

  return MAGMA_STATUS_OK;
}

// static
std::unique_ptr<msd::Driver> msd::Driver::MsdCreate() { return std::make_unique<MsdVsiDriver>(); }
