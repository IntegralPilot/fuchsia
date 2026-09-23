// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>

#include <sdk/lib/driver/component/cpp/composite_node_spec.h>
#include <sdk/lib/driver/component/cpp/node_add_args.h>
#include <soc/aml-s905d2/s905d2-gpio.h>
#include <soc/aml-s905d2/s905d2-hw.h>

#include "astro-gpios.h"
#include "astro.h"

// uncomment to disable LED blinky test
// #define GPIO_TEST

#define DECL_GPIO_PIN(x)     \
  {                          \
    {                        \
      .pin = (x), .name = #x \
    }                        \
  }

namespace {
namespace fpbus = fuchsia_hardware_platform_bus;

const std::vector<fpbus::Mmio> kGpioMmios{
    {{
        .base = S905D2_GPIO_BASE,
        .length = S905D2_GPIO_LENGTH,
    }},
    {{
        .base = S905D2_GPIO_AO_BASE,
        .length = S905D2_GPIO_AO_LENGTH,
    }},
    {{
        .base = S905D2_GPIO_INTERRUPT_BASE,
        .length = S905D2_GPIO_INTERRUPT_LENGTH,
    }},
};

const std::vector<fpbus::Irq> kGpioIrqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_GPIO_IRQ_0),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_GPIO_IRQ_1),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_GPIO_IRQ_2),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_GPIO_IRQ_3),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_GPIO_IRQ_4),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_GPIO_IRQ_5),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_GPIO_IRQ_6),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_GPIO_IRQ_7),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
};

#ifdef GPIO_TEST
zx_status_t CreateTestGpioPlatformDevice(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  const std::vector<fuchsia_hardware_pinimpl::Pin> kTestGpioPins = {
      // SYS_LED
      DECL_GPIO_PIN(S905D2_GPIOAO(11)),
      // JTAG Adapter Pin
      DECL_GPIO_PIN(S905D2_GPIOAO(6)),
  };

  const fuchsia_hardware_pinimpl::Metadata kMetadata{{.pins = kTestGpioPins}};

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist pin metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  std::vector<fpbus::Metadata> gpio_metadata{
      {{
          .id = fuchsia_hardware_pinimpl::Metadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };

  const fpbus::Node kTestGpioDev{{
      .name = "nelson-gpio-test",
      .vid = bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC,
      .pid = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC,
      .did = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_GPIO_TEST,
      .metadata = std::move(gpio_metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('GPIO');
  auto result = pbus.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, kTestGpioDev));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send NodeAdd request: %s", result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add node: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}
#endif

}  // namespace

namespace astro {

zx_status_t Astro::CreateGpioPlatformDevice() {
  // GPIOs to expose from generic GPIO driver.
  const std::vector<fuchsia_hardware_pinimpl::Pin> kGpioPins = {
      // For wifi.
      DECL_GPIO_PIN(S905D2_WIFI_SDIO_WAKE_HOST),
      // For display.
      DECL_GPIO_PIN(GPIO_PANEL_DETECT),
      DECL_GPIO_PIN(GPIO_LCD_RESET),
      // For touch screen.
      DECL_GPIO_PIN(GPIO_TOUCH_INTERRUPT),
      DECL_GPIO_PIN(GPIO_TOUCH_RESET),
      // For light sensor.
      DECL_GPIO_PIN(GPIO_LIGHT_INTERRUPT),
      // For audio.
      DECL_GPIO_PIN(GPIO_AUDIO_SOC_FAULT_L),
      DECL_GPIO_PIN(GPIO_SOC_AUDIO_EN),
      // For buttons.
      DECL_GPIO_PIN(GPIO_VOLUME_UP),
      DECL_GPIO_PIN(GPIO_VOLUME_DOWN),
      DECL_GPIO_PIN(GPIO_VOLUME_BOTH),
      DECL_GPIO_PIN(GPIO_MIC_PRIVACY),
      // For SDIO.
      DECL_GPIO_PIN(GPIO_SDIO_RESET),
      // For Bluetooth.
      DECL_GPIO_PIN(GPIO_SOC_WIFI_LPO_32k768),
      DECL_GPIO_PIN(GPIO_SOC_BT_REG_ON),
      // For lights.
      DECL_GPIO_PIN(GPIO_AMBER_LED),

      // Board revision GPIOs.
      DECL_GPIO_PIN(astro::GPIO_HW_ID0),
      DECL_GPIO_PIN(astro::GPIO_HW_ID1),
      DECL_GPIO_PIN(astro::GPIO_HW_ID2),
  };

  fuchsia_hardware_pinimpl::Metadata metadata{
      {.init_steps = std::move(gpio_init_steps_), .pins = kGpioPins}};
  gpio_init_steps_.clear();

  fit::result persisted_metadata = fidl::Persist(metadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist pin metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  std::vector<fpbus::Metadata> gpio_metadata{
      {{
          .id = fuchsia_hardware_pinimpl::Metadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };

  fpbus::Node gpio_dev{{
      .name = "gpio-controller-ff634400",
      .vid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC,
      .pid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2,
      .did = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_GPIO,
      .mmio = kGpioMmios,
      .irq = kGpioIrqs,
      .metadata = std::move(gpio_metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('GPIO');

  auto composite_spec = fuchsia_driver_framework::wire::CompositeNodeSpec::Builder(fidl_arena)
                            .name("gpio-controller-ff634400")
                            .Build();

  auto result =
      pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, gpio_dev), composite_spec);
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request: %s",
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add node: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

zx_status_t Astro::GpioInit() {
  gpio_init_steps_.push_back(GpioPull(GPIO_LIGHT_INTERRUPT, fuchsia_hardware_pin::Pull::kNone));
  gpio_init_steps_.push_back(fuchsia_hardware_pinimpl::InitStep::WithCall({{
      .pin = GPIO_LIGHT_INTERRUPT,
      .call = fuchsia_hardware_pinimpl::InitCall::WithBufferMode(
          fuchsia_hardware_gpio::BufferMode::kInput),
  }}));

  if (zx_status_t status = CreateGpioPlatformDevice(); status != ZX_OK) {
    zxlogf(ERROR, "Failed to create gpio platform device: %s", zx_status_get_string(status));
    return status;
  }

#ifdef GPIO_TEST
  if (zx_status_t status = CreateTestGpioPlatformDevice(pbus_); status != ZX_OK) {
    zxlogf(ERROR, "Failed to create test gpio platform device: %s", zx_status_get_string(status));
    return status;
  }
#endif

  return ZX_OK;
}

}  // namespace astro
