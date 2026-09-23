// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "msd_intel_driver.h"

#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>

#include "msd_intel_buffer.h"
#include "msd_intel_device.h"
#include "msd_intel_semaphore.h"

std::unique_ptr<msd::Device> MsdIntelDriver::MsdCreateDevice(msd::DeviceHandle* device_handle) {
  bool start_device_thread = (configure_flags() & MSD_DRIVER_CONFIG_TEST_NO_DEVICE_THREAD) == 0;

  std::unique_ptr<MsdIntelDevice> device =
      MsdIntelDevice::Create(device_handle, start_device_thread);

  if (!device)
    return DRETP(nullptr, "failed to create device");

  return device;
}

std::unique_ptr<msd::Buffer> MsdIntelDriver::MsdImportBuffer(zx::vmo vmo, uint64_t client_id) {
  auto buffer = MsdIntelBuffer::Import(std::move(vmo), client_id);
  if (!buffer)
    return DRETP(nullptr, "MsdIntelBuffer::Import failed");

  return std::make_unique<MsdIntelAbiBuffer>(std::move(buffer));
}

magma_status_t MsdIntelDriver::MsdImportSemaphore(zx::handle handle, uint64_t client_id,
                                                  uint64_t flags,
                                                  std::unique_ptr<msd::Semaphore>* semaphore_out) {
  auto semaphore = magma::PlatformSemaphore::Import(std::move(handle), flags);
  if (!semaphore)
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "couldn't import handle");

  semaphore->set_local_id(client_id);

  *semaphore_out = std::make_unique<MsdIntelAbiSemaphore>(
      std::shared_ptr<magma::PlatformSemaphore>(std::move(semaphore)));

  return MAGMA_STATUS_OK;
}

// static
std::unique_ptr<msd::Driver> msd::Driver::MsdCreate() { return std::make_unique<MsdIntelDriver>(); }
