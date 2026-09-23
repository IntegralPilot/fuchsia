// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/fit/thread_safety.h>
#include <lib/magma/platform/platform_handle.h>
#include <lib/magma/platform/zircon/zircon_platform_device_dfv2.h>
#include <lib/magma/platform/zircon/zircon_platform_logger_dfv2.h>
#include <lib/magma/platform/zircon/zircon_platform_status.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/sys_driver/magma_driver_base.h>
#include <lib/magma_service/sys_driver/magma_system_device.h>
#include <zircon/process.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <memory>

#include "parent_device_dfv2.h"

#if MAGMA_TEST_DRIVER
constexpr char kDriverName[] = "vsi-vip-test";

zx_status_t magma_indriver_test(ParentDeviceDfv2* device);

#else

constexpr char kDriverName[] = "vsi-vip";

#endif

class NpuDevice : public msd::MagmaDriverBase {
 public:
  constexpr static bool kServeUntrustedService = false;
  explicit NpuDevice() : msd::MagmaDriverBase(kDriverName, kServeUntrustedService) {}

  zx::result<> MagmaStart(fdf::DriverContext& context) override;

 private:
#if MAGMA_TEST_DRIVER
  msd::MagmaTestServer test_server_;
#endif

  ParentDeviceDfv2 parent_;
};

zx::result<> NpuDevice::MagmaStart(fdf::DriverContext& context) {
  parent_.incoming_ = incoming();
  std::lock_guard lock(magma_mutex());
  set_magma_driver(msd::Driver::MsdCreate());
  if (!magma_driver()) {
    DMESSAGE("Failed to create MagmaDriver");
    return zx::error(ZX_ERR_INTERNAL);
  }
#if MAGMA_TEST_DRIVER
  {
    DLOG("running magma indriver test");
    test_server_.set_unit_test_status(magma_indriver_test(&parent_));
    zx::result result = CreateTestService(test_server_);
    if (result.is_error()) {
      DMESSAGE("Failed to serve the TestService");
      return zx::error(ZX_ERR_INTERNAL);
    }
  }
#endif

  set_magma_system_device(msd::MagmaSystemDevice::Create(
      magma_driver(),
      magma_driver()->MsdCreateDevice(reinterpret_cast<msd::DeviceHandle*>(&parent_))));
  if (!magma_system_device()) {
    MAGMA_LOG(ERROR, "Failed to create device");
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  return zx::ok();
}

FUCHSIA_DRIVER_EXPORT2(NpuDevice);
