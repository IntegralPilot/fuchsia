// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/board/drivers/sherlock/sherlock.h"

#include <assert.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <array>

#include <bind/fuchsia/amlogic/platform/t931/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/google/platform/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>

#include "src/devices/board/drivers/sherlock/sherlock-gpios.h"

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

zx_status_t Sherlock::Create(void* ctx, zx_device_t* parent) {
  zx::result client = DdkConnectRuntimeProtocol<fpbus::Service::PlatformBus>(parent);
  if (client.is_error()) {
    return client.status_value();
  }

  fdf::WireSyncClient<fpbus::PlatformBus> pbus(std::move(client.value()));

  fbl::AllocChecker ac;
  auto board = fbl::make_unique_checked<Sherlock>(&ac, parent, pbus.TakeClientEnd());
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  {
    fuchsia_hardware_platform_bus::Service::InstanceHandler handler({
        .platform_bus = fit::bind_member<&Sherlock::Serve>(board.get()),
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
      ddk::MakeStrProperty(bind_fuchsia::PLATFORM_DEV_DID,
                           bind_fuchsia_google_platform::BIND_PLATFORM_DEV_DID_POST_INIT),
  };

  std::array<const char*, 1> fidl_service_offers{fuchsia_hardware_platform_bus::Service::Name};
  zx_status_t status =
      board->DdkAdd(ddk::DeviceAddArgs("sherlock")
                        .set_str_props(kBoardDriverProps)
                        .set_outgoing_dir(directory_endpoints->client.TakeChannel())
                        .set_runtime_service_offers(fidl_service_offers));
  if (status != ZX_OK) {
    return status;
  }

  // devmgr is now in charge of the device.
  [[maybe_unused]] auto* dummy = board.release();
  return ZX_OK;
}

int Sherlock::Start() {
  // Load protocol implementation drivers first.
  if (I2cInit() != ZX_OK) {
    zxlogf(ERROR, "I2cInit() failed");
  }

  if (SpiInit() != ZX_OK) {
    zxlogf(ERROR, "SpiInit() failed");
  }

  if (ThermalInit() != ZX_OK) {
    zxlogf(ERROR, "ThermalInit() failed");
  }

  if (EmmcInit() != ZX_OK) {
    zxlogf(ERROR, "EmmcInit() failed");
  }

  if (SdioInit() != ZX_OK) {
    zxlogf(ERROR, "SdioInit() failed");
  }

  if (BluetoothInit() != ZX_OK) {
    zxlogf(ERROR, "BluetoothInit() failed");
  }

  if (CameraInit() != ZX_OK) {
    zxlogf(ERROR, "CameraInit() failed");
  }

  if (AudioInit() != ZX_OK) {
    zxlogf(ERROR, "AudioInit() failed");
  }

  if (LightInit() != ZX_OK) {
    zxlogf(ERROR, "LightInit() failed");
    return -1;
  }

  if (PwmInit() != ZX_OK) {
    zxlogf(ERROR, "PwmInit() failed");
  }

  if (ButtonsInit() != ZX_OK) {
    zxlogf(ERROR, "ButtonsInit() failed");
  }

  // ClkInit() must be called after other subsystems that bind to clock have had a chance to add
  // their init steps.
  if (ClkInit() != ZX_OK) {
    zxlogf(ERROR, "ClkInit() failed");
    return -1;
  }

  if (AddPostInitDevice() != ZX_OK) {
    zxlogf(ERROR, "AddPostInitDevice() failed");
    return -1;
  }

  if (OtRadioInit() != ZX_OK) {
    zxlogf(ERROR, "OtRadioInit() failed");
  }

  // GpioInit() must be called after other subsystems that bind to GPIO have had a chance to add
  // their init steps.
  if (GpioInit() != ZX_OK) {
    zxlogf(ERROR, "GpioInit() failed");
    return -1;
  }

  if (RegistersInit() != ZX_OK) {
    zxlogf(ERROR, "RegistersInit() failed");
    return -1;
  }

  if (CpuInit() != ZX_OK) {
    zxlogf(ERROR, "CpuInit() failed\n");
  }

  if (CanvasInit() != ZX_OK) {
    zxlogf(ERROR, "CanvasInit() failed");
  }

  // Then the platform device drivers.
  if (UsbInit() != ZX_OK) {
    zxlogf(ERROR, "UsbInit() failed");
  }

  if (TeeInit() != ZX_OK) {
    zxlogf(ERROR, "TeeInit() failed");
  }

  if (VideoInit() != ZX_OK) {
    zxlogf(ERROR, "VideoInit() failed");
  }

  if (VideoEncInit() != ZX_OK) {
    zxlogf(ERROR, "VideoEncInit() failed");
  }

  if (HevcEncInit() != ZX_OK) {
    zxlogf(ERROR, "HevcEncInit() failed");
  }

  if (MaliInit() != ZX_OK) {
    zxlogf(ERROR, "MaliInit() failed");
  }

  if (NnaInit() != ZX_OK) {
    zxlogf(ERROR, "NnaInit() failed");
  }

  if (SecureMemInit() != ZX_OK) {
    zxlogf(ERROR, "SecureMbemInit failed");
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

  return 0;
}

void Sherlock::DdkInit(ddk::InitTxn txn) { txn.Reply(Start() == 0 ? ZX_OK : ZX_ERR_INTERNAL); }

void Sherlock::DdkRelease() { delete this; }

zx_status_t Sherlock::AddPostInitDevice() {
  constexpr uint64_t kAmlogicAltFunctionGpio = 0;

  constexpr std::array<uint32_t, 7> kPostInitGpios{
      bind_fuchsia_amlogic_platform_t931::GPIOA_PIN_ID_PIN_11,
      bind_fuchsia_amlogic_platform_t931::GPIOA_PIN_ID_PIN_12,
      bind_fuchsia_amlogic_platform_t931::GPIOC_PIN_ID_PIN_4,
      bind_fuchsia_amlogic_platform_t931::GPIOC_PIN_ID_PIN_5,
      bind_fuchsia_amlogic_platform_t931::GPIOC_PIN_ID_PIN_6,
      bind_fuchsia_amlogic_platform_t931::GPIOH_PIN_ID_PIN_0,
      bind_fuchsia_amlogic_platform_t931::GPIOH_PIN_ID_PIN_2,
  };

  const ddk::BindRule post_init_rules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.platform.bus.Service"),
      ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                              bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE),
      ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                              bind_fuchsia_google_platform::BIND_PLATFORM_DEV_DID_POST_INIT),
  };
  const device_bind_prop_t post_init_properties[] = {
      ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.platform.bus.Service"),
      ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_VID,
                        bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE),
      ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_DID,
                        bind_fuchsia_google_platform::BIND_PLATFORM_DEV_DID_POST_INIT),
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

    gpio_init_steps_.push_back(fuchsia_hardware_pinimpl::InitStep::WithCall({{
        .pin = pin,
        .call = fuchsia_hardware_pinimpl::InitCall::WithPinConfig({{
            .pull = fuchsia_hardware_pin::Pull::kNone,
            .function = kAmlogicAltFunctionGpio,
        }}),
    }}));
  }

  if (zx_status_t status = DdkAddCompositeNodeSpec("post-init", spec); status != ZX_OK) {
    zxlogf(ERROR, "Failed to add board info composite: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Sherlock::Create;
  return ops;
}();

}  // namespace sherlock

ZIRCON_DRIVER(sherlock, sherlock::driver_ops, "zircon", "0.1");
