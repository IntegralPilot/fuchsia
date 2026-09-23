// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <fuchsia/hardware/intelgpucore/cpp/banjo.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/magma/platform/platform_bus_mapper.h>
#include <lib/magma/platform/zircon/zircon_platform_status.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd_defs.h>
#include <lib/magma_service/sys_driver/magma_driver_base.h>
#include <lib/zx/channel.h>
#include <lib/zx/resource.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/process.h>
#include <zircon/types.h>

#if MAGMA_TEST_DRIVER
#include "msd_intel_pci_device.h"

constexpr char kDriverName[] = "magma-gpu-test";
zx_status_t magma_indriver_test(magma::PlatformPciDevice* platform_device);
#else
constexpr char kDriverName[] = "magma-gpu";
#endif

class IntelDevice : public msd::MagmaDriverBase {
 public:
  explicit IntelDevice() : msd::MagmaDriverBase(kDriverName) {}

  zx::result<> MagmaStart(fdf::DriverContext& context) override;

  void Stop(fdf::StopCompleter completer) override {
    magma::PlatformBusMapper::SetInfoResource(zx::resource{});
    msd::MagmaDriverBase::Stop(std::move(completer));
  }

 private:
  intel_gpu_core_protocol_t gpu_core_protocol_;
#if MAGMA_TEST_DRIVER
  msd::MagmaTestServer test_server_;
#endif
};

zx::result<> IntelDevice::MagmaStart(fdf::DriverContext& context) {
  zx::result info_resource = GetInfoResource();
  // Info resource may not be available on user builds.
  if (info_resource.is_ok()) {
    magma::PlatformBusMapper::SetInfoResource(std::move(*info_resource));
  }

  std::lock_guard lock(magma_mutex());
  set_magma_driver(msd::Driver::MsdCreate());
  if (!magma_driver()) {
    DMESSAGE("Failed to create MagmaDriver");
    return zx::error(ZX_ERR_INTERNAL);
  }
  DLOG("Created device %p", magma_system_device());

  zx::result banjo = compat::ConnectBanjo<ddk::IntelGpuCoreProtocolClient>(incoming());
  if (banjo.is_error()) {
    MAGMA_LOG(ERROR, "Failed to connect to banjo %s", banjo.status_string());
    return banjo.take_error();
  }
  banjo->GetProto(&gpu_core_protocol_);

#if MAGMA_TEST_DRIVER
  DLOG("running magma indriver test");
  {
    auto platform_device = MsdIntelPciDevice::CreateShim(&gpu_core_protocol_);
    test_server_.set_unit_test_status(magma_indriver_test(platform_device.get()));
    zx::result result = CreateTestService(test_server_);
    if (result.is_error()) {
      DMESSAGE("Failed to serve the TestService");
      return zx::error(ZX_ERR_INTERNAL);
    }
  }
#endif

  set_magma_system_device(msd::MagmaSystemDevice::Create(
      magma_driver(),
      magma_driver()->MsdCreateDevice(reinterpret_cast<msd::DeviceHandle*>(&gpu_core_protocol_))));
  if (!magma_system_device()) {
    MAGMA_LOG(ERROR, "Failed to create device");
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  return zx::ok();
}

FUCHSIA_DRIVER_EXPORT2(IntelDevice);
