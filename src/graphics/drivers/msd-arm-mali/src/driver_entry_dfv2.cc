// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpu.mali/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.gpu.mali/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/power/cpp/suspend.h>
#include <lib/fit/thread_safety.h>
#include <lib/magma/platform/platform_bus_mapper.h>
#include <lib/magma/platform/zircon/zircon_platform_logger_dfv2.h>
#include <lib/magma/platform/zircon/zircon_platform_status.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/sys_driver/magma_driver_base.h>

#include "msd_arm_device.h"
#include "parent_device_dfv2.h"

#if MAGMA_TEST_DRIVER
constexpr char kDriverName[] = "mali-test";

zx_status_t magma_indriver_test(ParentDevice* device);

#else
constexpr char kDriverName[] = "mali";
#endif

class MaliDriver : public msd::MagmaDriverBase, public fdf_power::Suspendable<MaliDriver> {
 public:
  explicit MaliDriver() : msd::MagmaDriverBase(kDriverName) {}

  // Returns the power element runner server end taken from DriverContext during MagmaStart.
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> take_power_element_runner() {
    return std::move(power_element_runner_);
  }

  // Returns whether power suspension is enabled for this driver instance.
  // Suspend will be enabled if the platform is suspend-enabled and the parent,
  // if one exists, says suspend is enabled.
  bool SuspendEnabled() override {
    // If there is a parent device, it must agree that suspend is enabled.
    if (parent_device_) {
      return parent_device_->suspend_enabled() && df_suspend_enabled_;
    }

    // There is no parent device, so return what we think.
    return df_suspend_enabled_;
  }

  // Invoked by fdf_power::Suspendable when system requests power suspension.
  // Delegates to msd::Device::MsdSuspend and invokes completer asynchronously once hardware
  // and driver power manager state transitions complete.
  void Suspend(fdf_power::SuspendCompleter completer) override {
    std::lock_guard lock(magma_mutex());
    if (!magma_system_device()) {
      completer();
      return;
    }
    magma_system_device()->msd_dev()->MsdSuspend(
        [completer = std::move(completer)](magma_status_t status) mutable { completer(); });
  }

  // Invoked by fdf_power::Suspendable when system requests power resumption.
  // Delegates to msd::Device::MsdResume and invokes completer asynchronously once hardware
  // power restoration completes.
  void Resume(fdf_power::ResumeCompleter completer) override {
    std::lock_guard lock(magma_mutex());
    if (!magma_system_device()) {
      completer();
      return;
    }
    magma_system_device()->msd_dev()->MsdResume(
        [completer = std::move(completer)](magma_status_t status) mutable { completer(); });
  }

  zx::result<> MagmaStart(fdf::DriverContext& context) override {
    power_element_runner_ = context.take_power_element_runner();
    zx::result info_resource = GetInfoResource();
    // Info resource may not be available on user builds.
    if (info_resource.is_ok()) {
      magma::PlatformBusMapper::SetInfoResource(std::move(*info_resource));
    }

    df_suspend_enabled_ = context.has_power_args();

    parent_device_ = ParentDeviceDFv2::Create(incoming(), df_suspend_enabled_);
    if (!parent_device_) {
      MAGMA_LOG(ERROR, "Failed to create ParentDeviceDFv2");
      return zx::error(ZX_ERR_INTERNAL);
    }

    std::lock_guard lock(magma_mutex());

    set_magma_driver(msd::Driver::MsdCreate());
    if (!magma_driver()) {
      MAGMA_LOG(ERROR, "Failed to create MagmaDriver");
      return zx::error(ZX_ERR_INTERNAL);
    }

#if MAGMA_TEST_DRIVER
    {
      test_server_.set_unit_test_status(magma_indriver_test(parent_device_.get()));
      zx::result result = CreateTestService(test_server_);
      if (result.is_error()) {
        MAGMA_LOG(ERROR, "Failed to serve the TestService");
        return zx::error(ZX_ERR_INTERNAL);
      }
    }
#endif

    set_magma_system_device(msd::MagmaSystemDevice::Create(
        magma_driver(), magma_driver()->MsdCreateDevice(parent_device_->ToDeviceHandle())));
    if (!magma_system_device()) {
      MAGMA_LOG(ERROR, "Failed to create MagmaSystemDevice");
      return zx::error(ZX_ERR_INTERNAL);
    }

    auto status = InitializeSuspend(dispatcher(), *incoming(), kDriverName);
    if (status.is_error()) {
      MAGMA_LOG(ERROR, "MaliDriver::MagmaStat Initialized suspend failed! %d",
                status.error_value());
      return status.take_error();
    }

    return zx::ok();
  }

  void Stop(fdf::StopCompleter completer) override {
    magma::PlatformBusMapper::SetInfoResource(zx::resource{});
    msd::MagmaDriverBase::Stop(std::move(completer));
  }

 private:
  std::unique_ptr<ParentDeviceDFv2> parent_device_;
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> power_element_runner_;
  // Whether the driver framework is suspend-enabled, based on whether it sent
  // power args in the driver's start args.
  bool df_suspend_enabled_ = false;

#if MAGMA_TEST_DRIVER
  msd::MagmaTestServer test_server_;
#endif
};

FUCHSIA_DRIVER_EXPORT2(MaliDriver);
