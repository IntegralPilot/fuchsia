// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/platform/cpp/bind.h>

class ParentDriver : public fdf::DriverBase2 {
 public:
  ParentDriver() : fdf::DriverBase2("test-parent") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
    zx::result pdev = incoming->Connect<fuchsia_hardware_platform_device::Service::Device>();
    if (pdev.is_error()) {
      fdf::error("Failed to connect to platform device: {}", pdev.status_string());
      return pdev.take_error();
    }

    fidl::WireSyncClient<fuchsia_hardware_platform_device::Device> client(std::move(pdev.value()));
    auto irq_result = client->GetInterruptById(0, 0);
    if (!irq_result.ok()) {
      fdf::error("Call to GetInterruptById failed: {}", irq_result.error().FormatDescription());
      return zx::error(irq_result.error().status());
    }
    if (irq_result->is_error()) {
      fdf::error("GetInterruptById failed: {}", zx_status_get_string(irq_result->error_value()));
      return zx::error(irq_result->error_value());
    }

    zx::interrupt irq = std::move(irq_result->value()->irq);
    // The test interrupt controller driver should have triggered the interrupt object before
    // returning it to us.
    zx_status_t status = irq.wait(nullptr);
    if (status != ZX_OK) {
      fdf::error("zx_interrupt_wait failed: {}", zx_status_get_string(status));
      return zx::error(status);
    }

    auto child_1_properties =
        std::vector{fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                                       bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
                    fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                                       bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_PBUS_TEST),
                    fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                                       bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_CHILD_1)};
    zx::result result = AddChild("child-1", child_1_properties, {});
    if (result.is_error()) {
      return result.take_error();
    }

    auto properties = std::vector{
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_PBUS_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_PARENT_SPEC),
    };
    result = AddChild("node_a", properties, {});
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }
};

FUCHSIA_DRIVER_EXPORT2(ParentDriver);
