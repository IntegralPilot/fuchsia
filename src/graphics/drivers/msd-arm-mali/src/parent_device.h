// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_PARENT_DEVICE_H_
#define SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_PARENT_DEVICE_H_

#include <fidl/fuchsia.hardware.gpu.mali/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/magma/platform/platform_device.h>
#include <lib/magma/platform/platform_interrupt.h>
#include <lib/magma/platform/platform_mmio.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/status.h>
#include <lib/magma_service/msd.h>

#include <chrono>
#include <memory>

class ParentDevice {
 public:
  virtual ~ParentDevice() { DLOG("ParentDevice dtor"); }

  msd::DeviceHandle* ToDeviceHandle() { return reinterpret_cast<msd::DeviceHandle*>(this); }

  virtual zx::bti GetBusTransactionInitiator() = 0;

  virtual bool SetThreadRole(const char* role_name) = 0;

  // Map an MMIO listed at |index| in the platform device
  virtual std::unique_ptr<magma::PlatformMmio> CpuMapMmio(unsigned int index) = 0;

  // Register an interrupt listed at |index| in the platform device.
  virtual std::unique_ptr<magma::PlatformInterrupt> RegisterInterrupt(unsigned int index) = 0;

  virtual zx::result<fdf::ClientEnd<fuchsia_hardware_gpu_mali::ArmMali>>
  ConnectToMaliRuntimeProtocol() = 0;
  virtual bool suspend_enabled() = 0;

  virtual magma::PlatformDevice* GetPlatformDevice() = 0;

  virtual std::shared_ptr<fdf::Namespace> incoming() = 0;
};

#endif  // SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_PARENT_DEVICE_H_
