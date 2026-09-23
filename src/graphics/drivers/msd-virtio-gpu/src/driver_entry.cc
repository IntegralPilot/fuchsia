// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.virtio/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/magma/platform/platform_bus_mapper.h>
#include <lib/magma_service/msd.h>
#include <lib/magma_service/sys_driver/magma_driver_base.h>

#include "virtio_gpu_control.h"

class VirtioDriver : public msd::MagmaDriverBase, VirtioGpuControlFidl {
 public:
  explicit VirtioDriver() : msd::MagmaDriverBase("virtio") {}

  zx::result<> MagmaStart(fdf::DriverContext& context) override;

  void Stop(fdf::StopCompleter completer) override {
    fdf::info("VirtioDevice::Stop");
    completer(zx::ok());
  }
};

zx::result<> VirtioDriver::MagmaStart(fdf::DriverContext& context) {
  fdf::info("VirtioDevice::Start");

  zx::result info_resource = GetInfoResource();
  // Info resource may not be available on user builds.
  if (info_resource.is_ok()) {
    magma::PlatformBusMapper::SetInfoResource(std::move(*info_resource));
  }

  auto result = VirtioGpuControlFidl::Init(incoming());
  if (result.is_error()) {
    fdf::error("VirtioGpuControlFidl::Init failed: {}", result);
    return result.take_error();
  }

  {
    std::lock_guard lock(magma_mutex());

    set_magma_driver(msd::Driver::MsdCreate());

    if (!magma_driver()) {
      fdf::error("msd::Driver::Create failed");
      return zx::error(ZX_ERR_INTERNAL);
    }

    auto msd_device = magma_driver()->MsdCreateDevice(static_cast<VirtioGpuControl*>(this));

    set_magma_system_device(msd::MagmaSystemDevice::Create(magma_driver(), std::move(msd_device)));

    if (!magma_system_device()) {
      fdf::error("msd::MagmaSystemDevice::Create failed");
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  return zx::ok();
}

FUCHSIA_DRIVER_EXPORT2(VirtioDriver);
