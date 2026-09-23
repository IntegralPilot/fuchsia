// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/msd-arm-mali/src/msd_arm_driver.h"

#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>

#include "src/graphics/drivers/msd-arm-mali/src/msd_arm_device.h"

MsdArmDriver::MsdArmDriver() {
  magic_ = kMagic;
  root_node_ = inspector_.GetRoot().CreateChild("msd-arm-mali");
}

std::unique_ptr<MsdArmDriver> MsdArmDriver::MsdCreate() {
  return std::unique_ptr<MsdArmDriver>(new MsdArmDriver());
}

void MsdArmDriver::Destroy(MsdArmDriver* drv) { delete drv; }

std::optional<inspect::Inspector> MsdArmDriver::MsdDuplicateInspector() { return inspector_; }

std::unique_ptr<msd::Device> MsdArmDriver::MsdCreateDevice(msd::DeviceHandle* device_handle) {
  bool start_device_thread = (configure_flags() & MSD_DRIVER_CONFIG_TEST_NO_DEVICE_THREAD) == 0;

  std::unique_ptr<MsdArmDevice> device =
      MsdArmDevice::Create(device_handle, start_device_thread, &root_node());
  if (!device)
    return DRETP(nullptr, "failed to create device");
  return device;
}

std::unique_ptr<msd::Buffer> MsdArmDriver::MsdImportBuffer(zx::vmo vmo, uint64_t client_id) {
  auto buffer = MsdArmBuffer::Import(std::move(vmo), client_id);
  if (!buffer)
    return DRETP(nullptr, "MsdArmBuffer::Create failed");
  return std::make_unique<MsdArmAbiBuffer>(std::move(buffer));
}

magma_status_t MsdArmDriver::MsdImportSemaphore(zx::handle handle, uint64_t client_id,
                                                uint64_t flags,
                                                std::unique_ptr<msd::Semaphore>* semaphore_out) {
  auto semaphore = magma::PlatformSemaphore::Import(std::move(handle), flags);
  if (!semaphore)
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "couldn't import semaphore handle");

  semaphore->set_local_id(client_id);

  *semaphore_out = std::make_unique<MsdArmAbiSemaphore>(
      std::shared_ptr<magma::PlatformSemaphore>(std::move(semaphore)));

  return MAGMA_STATUS_OK;
}

std::unique_ptr<MsdArmDevice> MsdArmDriver::CreateDeviceForTesting(
    ParentDevice* parent_device, std::unique_ptr<magma::PlatformBusMapper> bus_mapper) {
  auto device = std::make_unique<MsdArmDevice>();
  device->set_inspect(root_node_.CreateChild("device"));
  device->set_assume_reset_happened(true);

  if (!device->Init(parent_device, std::move(bus_mapper)))
    return DRETF(nullptr, "Failed to create device");
  return device;
}

// static
std::unique_ptr<msd::Driver> msd::Driver::MsdCreate() { return std::make_unique<MsdArmDriver>(); }
