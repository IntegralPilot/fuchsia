// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/devices/board/drivers/nelson/nelson.h"

#include <assert.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <array>

#include <bind/fuchsia/amlogic/platform/s905d3/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/google/platform/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>

#include "src/devices/board/drivers/nelson/nelson-gpios.h"

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;
namespace fhgpio = fuchsia_hardware_gpio;

int Nelson::Thread() {
  zx_status_t status;

  if ((status = I2cInit()) != ZX_OK) {
    zxlogf(ERROR, "I2cInit failed: %d", status);
  }

  if ((status = SpiInit()) != ZX_OK) {
    zxlogf(ERROR, "SpiInit failed: %d", status);
  }

  if ((status = EmmcInit()) != ZX_OK) {
    zxlogf(ERROR, "EmmcInit() failed: %d", status);
  }

  if ((status = SdioInit()) != ZX_OK) {
    zxlogf(ERROR, "SdioInit failed: %d", status);
  }

  if ((status = LightInit()) != ZX_OK) {
    zxlogf(ERROR, "LightInit failed: %d", status);
  }

  if ((status = ThermalInit()) != ZX_OK) {
    zxlogf(ERROR, "ThermalInit failed: %d", status);
  }

  if ((status = AudioInit()) != ZX_OK) {
    zxlogf(ERROR, "AudioInit failed: %d", status);
  }

  if (OtRadioInit() != ZX_OK) {
    zxlogf(ERROR, "OtRadioInit failed");
  }

  if ((status = BluetoothInit()) != ZX_OK) {
    zxlogf(ERROR, "BluetoothInit failed: %d", status);
  }

  if ((status = PwmInit()) != ZX_OK) {
    zxlogf(ERROR, "PwmInit failed: %d", status);
  }

  if ((status = ButtonsInit()) != ZX_OK) {
    zxlogf(ERROR, "ButtonsInit failed: %d", status);
  }

  if ((status = BrownoutProtectionInit()) != ZX_OK) {
    zxlogf(ERROR, "BrownoutProtectionInit failed: %d", status);
  }

  // ClkInit() must be called after other subsystems that bind to clock have had a chance to add
  // their init steps.
  if ((status = ClkInit()) != ZX_OK) {
    zxlogf(ERROR, "ClkInit failed: %d", status);
  }
  clock_init_steps_.clear();

  if ((status = AddPostInitDevice()) != ZX_OK) {
    zxlogf(ERROR, "%s: AddPostInitDevice() failed: %d", __func__, status);
    return status;
  }

  // GpioInit() must be called after other subsystems that bind to GPIO have had a chance to add
  // their init steps.
  if ((status = GpioInit()) != ZX_OK) {
    zxlogf(ERROR, "%s: GpioInit() failed: %d", __func__, status);
    return status;
  }
  gpio_init_steps_.clear();

  if ((status = RegistersInit()) != ZX_OK) {
    zxlogf(ERROR, "RegistersInit failed: %d", status);
  }

  if ((status = CpuInit()) != ZX_OK) {
    zxlogf(ERROR, "CpuInit failed: %d", status);
  }

  if ((status = MaliInit()) != ZX_OK) {
    zxlogf(ERROR, "MaliInit failed: %d", status);
  }

  if ((status = UsbInit()) != ZX_OK) {
    zxlogf(ERROR, "UsbInit failed: %d", status);
  }

  if ((status = CanvasInit()) != ZX_OK) {
    zxlogf(ERROR, "CanvasInit failed: %d", status);
  }

  if ((status = TeeInit()) != ZX_OK) {
    zxlogf(ERROR, "TeeInit failed: %d", status);
  }

  if ((status = VideoInit()) != ZX_OK) {
    zxlogf(ERROR, "VideoInit failed: %d", status);
  }

  if ((status = SecureMemInit()) != ZX_OK) {
    zxlogf(ERROR, "SecureMemInit failed: %d", status);
  }

  if ((status = PowerInit()) != ZX_OK) {
    zxlogf(ERROR, "PowerInit failed: %d", status);
  }

  if ((status = NnaInit()) != ZX_OK) {
    zxlogf(ERROR, "NnaInit failed: %d", status);
  }

  if (RamCtlInit() != ZX_OK) {
    zxlogf(ERROR, "RamCtlInit failed");
  }

  if (auto result = AdcInit(); result.is_error()) {
    zxlogf(ERROR, "AdcInit failed: %d", result.error_value());
  }

  if (ThermistorInit() != ZX_OK) {
    zxlogf(ERROR, "ThermistorInit failed");
  }

  ZX_ASSERT_MSG(clock_init_steps_.empty(), "Clock init steps added but not applied");
  ZX_ASSERT_MSG(gpio_init_steps_.empty(), "GPIO init steps added but not applied");

  return ZX_OK;
}

zx_status_t Nelson::Start() {
  int rc = thrd_create_with_name(
      &thread_, [](void* arg) -> int { return reinterpret_cast<Nelson*>(arg)->Thread(); }, this,
      "nelson-start-thread");
  if (rc != thrd_success) {
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

void Nelson::DdkRelease() { delete this; }

zx_status_t Nelson::Create(void* ctx, zx_device_t* parent) {
  zx::result client = DdkConnectRuntimeProtocol<fpbus::Service::PlatformBus>(parent);
  if (client.is_error()) {
    return client.status_value();
  }

  fbl::AllocChecker ac;
  auto board = fbl::make_unique_checked<Nelson>(&ac, parent, std::move(client.value()));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  {
    fuchsia_hardware_platform_bus::Service::InstanceHandler handler({
        .platform_bus = fit::bind_member<&Nelson::Serve>(board.get()),
    });
    auto result =
        board->outgoing_.AddService<fuchsia_hardware_platform_bus::Service>(std::move(handler));
    if (result.is_error()) {
      zxlogf(ERROR, "AddService failed: %s", result.status_string());
      return result.error_value();
    }
  }

  auto directory_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (directory_endpoints.is_error()) {
    return directory_endpoints.status_value();
  }

  {
    auto result = board->outgoing_.Serve(std::move(directory_endpoints->server));
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to serve the outgoing directory: %s", result.status_string());
      return result.error_value();
    }
  }

  const zx_device_str_prop_t kBoardDriverProps[] = {
      ddk::MakeStrProperty(bind_fuchsia::PLATFORM_DEV_VID,
                           bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE),
      ddk::MakeStrProperty(bind_fuchsia::PLATFORM_DEV_PID,
                           bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_NELSON),
      ddk::MakeStrProperty(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, 1u),
  };

  const char* fidl_service_offers[] = {fuchsia_hardware_platform_bus::Service::Name};
  zx_status_t status =
      board->DdkAdd(ddk::DeviceAddArgs("nelson")
                        .set_str_props(kBoardDriverProps)
                        .set_outgoing_dir(directory_endpoints->client.TakeChannel())
                        .set_runtime_service_offers({fidl_service_offers, 1})
                        .forward_metadata(parent, DEVICE_METADATA_BOARD_PRIVATE));
  if (status != ZX_OK) {
    return status;
  }

  // Start up our protocol helpers and platform devices.
  status = board->Start();
  if (status == ZX_OK) {
    // devmgr is now in charge of the device.
    [[maybe_unused]] auto* dummy = board.release();
  }
  return status;
}

zx_status_t Nelson::AddPostInitDevice() {
  constexpr std::array<uint32_t, 9> kPostInitGpios{
      bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_7,
      bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_8,
      bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_3,
      bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_0,
      bind_fuchsia_amlogic_platform_s905d3::GPIOAO_PIN_ID_PIN_4,
      bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_11,
      bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_12,
      bind_fuchsia_amlogic_platform_s905d3::GPIOH_PIN_ID_PIN_8,
      bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_4,
  };

  const ddk::BindRule post_init_rules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.platform.bus.Service"),
      ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                              bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE),
      ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                              bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_NELSON),
      ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, 1u),
  };
  const device_bind_prop_t post_init_properties[] = {
      ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.platform.bus.Service"),
      ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_VID,
                        bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE),
      ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_PID,
                        bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_NELSON),
      ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, 1u),
  };

  auto spec = ddk::CompositeNodeSpec(post_init_rules, post_init_properties);

  const ddk::BindRule gpio_init_rules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };
  const device_bind_prop_t gpio_init_properties[] = {
      ddk::MakeProperty(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };
  spec.AddParentSpec(gpio_init_rules, gpio_init_properties);

  for (const uint32_t pin : kPostInitGpios) {
    const ddk::BindRule gpio_rules[] = {
        ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
        ddk::MakeAcceptBindRule(bind_fuchsia::ID, pin),
    };
    const device_bind_prop_t gpio_properties[] = {
        ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
        ddk::MakeProperty(bind_fuchsia::ID, pin),
    };
    spec.AddParentSpec(gpio_rules, gpio_properties);

    gpio_init_steps_.push_back(GpioPull(pin, fuchsia_hardware_pin::Pull::kNone));
  }

  if (zx_status_t status = DdkAddCompositeNodeSpec("post-init", spec); status != ZX_OK) {
    zxlogf(ERROR, "Failed to add board info composite: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

static zx_driver_ops_t nelson_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Nelson::Create;
  return ops;
}();

}  // namespace nelson

ZIRCON_DRIVER(nelson, nelson::nelson_driver_ops, "zircon", "0.1");
