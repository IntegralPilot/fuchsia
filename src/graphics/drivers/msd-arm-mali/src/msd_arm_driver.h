// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_ARM_DRIVER_H
#define MSD_ARM_DRIVER_H

#include <lib/inspect/cpp/inspect.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

#include <memory>

#include "parent_device.h"
#include "src/graphics/drivers/msd-arm-mali/src/msd_arm_device.h"

class MsdArmDriver : public msd::Driver {
 public:
  MsdArmDriver();
  virtual ~MsdArmDriver() {}

  static std::unique_ptr<MsdArmDriver> MsdCreate();
  static void Destroy(MsdArmDriver* drv);

  // msd::Driver implementation.
  void MsdConfigure(uint32_t flags) override { configure_flags_ = flags; }
  std::optional<inspect::Inspector> MsdDuplicateInspector() override;
  std::unique_ptr<msd::Device> MsdCreateDevice(msd::DeviceHandle* device_handle) override;
  std::unique_ptr<msd::Buffer> MsdImportBuffer(zx::vmo vmo, uint64_t client_id) override;
  magma_status_t MsdImportSemaphore(zx::handle handle, uint64_t client_id, uint64_t flags,
                                    std::unique_ptr<msd::Semaphore>* out) override;

  uint32_t configure_flags() { return configure_flags_; }

  inspect::Node& root_node() { return root_node_; }

  std::unique_ptr<MsdArmDevice> CreateDeviceForTesting(
      ParentDevice* parent_device, std::unique_ptr<magma::PlatformBusMapper> bus_mapper);

 private:
  static const uint32_t kMagic = 0x64726976;  //"driv"
  uint32_t magic_;

  uint32_t configure_flags_ = 0;
  inspect::Inspector inspector_;
  // Available under the bootstrap/driver_manager:root/msd-arm-mali selector or
  // in /dev/diagnotics/class/gpu/000.inspect
  inspect::Node root_node_;
};

#endif  // MSD_ARM_DRIVER_H
