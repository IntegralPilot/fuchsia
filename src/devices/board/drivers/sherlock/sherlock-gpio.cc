// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>

#include <sdk/lib/driver/component/cpp/composite_node_spec.h>
#include <sdk/lib/driver/component/cpp/node_add_args.h>
#include <soc/aml-t931/t931-gpio.h>
#include <soc/aml-t931/t931-hw.h>

#include "sherlock-gpios.h"
#include "sherlock.h"

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
        .base = T931_GPIO_BASE,
        .length = T931_GPIO_LENGTH,
    }},
    {{
        .base = T931_GPIO_AO_BASE,
        .length = T931_GPIO_AO_LENGTH,
    }},
    {{
        .base = T931_GPIO_INTERRUPT_BASE,
        .length = T931_GPIO_INTERRUPT_LENGTH,
    }},
};

const std::vector<fpbus::Irq> kGpioIrqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_GPIO_IRQ_0),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_GPIO_IRQ_1),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_GPIO_IRQ_2),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_GPIO_IRQ_3),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_GPIO_IRQ_4),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_GPIO_IRQ_5),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_GPIO_IRQ_6),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_GPIO_IRQ_7),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
};

// GPIOs to expose from generic GPIO driver. Do not expose bank C GPIOs here, as they are managed by
// a separate device below.
#ifdef FACTORY_BUILD
#define GPIO_PIN_COUNT 120
const fuchsia_hardware_pinimpl::Pin kGpioPins[] = {
    DECL_GPIO_PIN(T931_GPIOZ(0)),     DECL_GPIO_PIN(T931_GPIOZ(1)),
    DECL_GPIO_PIN(T931_GPIOZ(2)),     DECL_GPIO_PIN(T931_GPIOZ(3)),
    DECL_GPIO_PIN(T931_GPIOZ(4)),     DECL_GPIO_PIN(T931_GPIOZ(5)),
    DECL_GPIO_PIN(T931_GPIOZ(6)),     DECL_GPIO_PIN(T931_GPIOZ(7)),
    DECL_GPIO_PIN(T931_GPIOZ(8)),     DECL_GPIO_PIN(T931_GPIOZ(9)),
    DECL_GPIO_PIN(T931_GPIOZ(10)),    DECL_GPIO_PIN(T931_GPIOZ(11)),
    DECL_GPIO_PIN(T931_GPIOZ(12)),    DECL_GPIO_PIN(T931_GPIOZ(13)),
    DECL_GPIO_PIN(T931_GPIOZ(14)),    DECL_GPIO_PIN(T931_GPIOZ(15)),
    DECL_GPIO_PIN(T931_GPIOA(0)),     DECL_GPIO_PIN(T931_GPIOA(1)),
    DECL_GPIO_PIN(T931_GPIOA(2)),     DECL_GPIO_PIN(T931_GPIOA(3)),
    DECL_GPIO_PIN(T931_GPIOA(4)),     DECL_GPIO_PIN(T931_GPIOA(5)),
    DECL_GPIO_PIN(T931_GPIOA(6)),     DECL_GPIO_PIN(T931_GPIOA(7)),
    DECL_GPIO_PIN(T931_GPIOA(8)),     DECL_GPIO_PIN(T931_GPIOA(9)),
    DECL_GPIO_PIN(T931_GPIOA(10)),    DECL_GPIO_PIN(T931_GPIOA(11)),
    DECL_GPIO_PIN(T931_GPIOA(12)),    DECL_GPIO_PIN(T931_GPIOA(13)),
    DECL_GPIO_PIN(T931_GPIOA(14)),    DECL_GPIO_PIN(T931_GPIOA(15)),
    DECL_GPIO_PIN(T931_GPIOBOOT(0)),  DECL_GPIO_PIN(T931_GPIOBOOT(1)),
    DECL_GPIO_PIN(T931_GPIOBOOT(2)),  DECL_GPIO_PIN(T931_GPIOBOOT(3)),
    DECL_GPIO_PIN(T931_GPIOBOOT(4)),  DECL_GPIO_PIN(T931_GPIOBOOT(5)),
    DECL_GPIO_PIN(T931_GPIOBOOT(6)),  DECL_GPIO_PIN(T931_GPIOBOOT(7)),
    DECL_GPIO_PIN(T931_GPIOBOOT(8)),  DECL_GPIO_PIN(T931_GPIOBOOT(9)),
    DECL_GPIO_PIN(T931_GPIOBOOT(10)), DECL_GPIO_PIN(T931_GPIOBOOT(11)),
    DECL_GPIO_PIN(T931_GPIOBOOT(12)), DECL_GPIO_PIN(T931_GPIOBOOT(13)),
    DECL_GPIO_PIN(T931_GPIOBOOT(14)), DECL_GPIO_PIN(T931_GPIOBOOT(15)),
    DECL_GPIO_PIN(T931_GPIOX(0)),     DECL_GPIO_PIN(T931_GPIOX(1)),
    DECL_GPIO_PIN(T931_GPIOX(2)),     DECL_GPIO_PIN(T931_GPIOX(3)),
    DECL_GPIO_PIN(T931_GPIOX(4)),     DECL_GPIO_PIN(T931_GPIOX(5)),
    DECL_GPIO_PIN(T931_GPIOX(6)),     DECL_GPIO_PIN(T931_GPIOX(7)),
    DECL_GPIO_PIN(T931_GPIOX(8)),     DECL_GPIO_PIN(T931_GPIOX(9)),
    DECL_GPIO_PIN(T931_GPIOX(10)),    DECL_GPIO_PIN(T931_GPIOX(11)),
    DECL_GPIO_PIN(T931_GPIOX(12)),    DECL_GPIO_PIN(T931_GPIOX(13)),
    DECL_GPIO_PIN(T931_GPIOX(14)),    DECL_GPIO_PIN(T931_GPIOX(15)),
    DECL_GPIO_PIN(T931_GPIOX(16)),    DECL_GPIO_PIN(T931_GPIOX(17)),
    DECL_GPIO_PIN(T931_GPIOX(18)),    DECL_GPIO_PIN(T931_GPIOX(19)),
    DECL_GPIO_PIN(T931_GPIOX(20)),    DECL_GPIO_PIN(T931_GPIOX(21)),
    DECL_GPIO_PIN(T931_GPIOX(22)),    DECL_GPIO_PIN(T931_GPIOX(23)),
    DECL_GPIO_PIN(T931_GPIOH(0)),     DECL_GPIO_PIN(T931_GPIOH(1)),
    DECL_GPIO_PIN(T931_GPIOH(2)),     DECL_GPIO_PIN(T931_GPIOH(3)),
    DECL_GPIO_PIN(T931_GPIOH(4)),     DECL_GPIO_PIN(T931_GPIOH(5)),
    DECL_GPIO_PIN(T931_GPIOH(6)),     DECL_GPIO_PIN(T931_GPIOH(7)),
    DECL_GPIO_PIN(T931_GPIOH(8)),     DECL_GPIO_PIN(T931_GPIOH(9)),
    DECL_GPIO_PIN(T931_GPIOH(10)),    DECL_GPIO_PIN(T931_GPIOH(11)),
    DECL_GPIO_PIN(T931_GPIOH(12)),    DECL_GPIO_PIN(T931_GPIOH(13)),
    DECL_GPIO_PIN(T931_GPIOH(14)),    DECL_GPIO_PIN(T931_GPIOH(15)),
    DECL_GPIO_PIN(T931_GPIOAO(0)),    DECL_GPIO_PIN(T931_GPIOAO(1)),
    DECL_GPIO_PIN(T931_GPIOAO(2)),    DECL_GPIO_PIN(T931_GPIOAO(3)),
    DECL_GPIO_PIN(T931_GPIOAO(4)),    DECL_GPIO_PIN(T931_GPIOAO(5)),
    DECL_GPIO_PIN(T931_GPIOAO(6)),    DECL_GPIO_PIN(T931_GPIOAO(7)),
    DECL_GPIO_PIN(T931_GPIOAO(8)),    DECL_GPIO_PIN(T931_GPIOAO(9)),
    DECL_GPIO_PIN(T931_GPIOAO(10)),   DECL_GPIO_PIN(T931_GPIOAO(11)),
    DECL_GPIO_PIN(T931_GPIOAO(12)),   DECL_GPIO_PIN(T931_GPIOAO(13)),
    DECL_GPIO_PIN(T931_GPIOAO(14)),   DECL_GPIO_PIN(T931_GPIOAO(15)),
    DECL_GPIO_PIN(T931_GPIOE(0)),     DECL_GPIO_PIN(T931_GPIOE(1)),
    DECL_GPIO_PIN(T931_GPIOE(2)),     DECL_GPIO_PIN(T931_GPIOE(3)),
    DECL_GPIO_PIN(T931_GPIOE(4)),     DECL_GPIO_PIN(T931_GPIOE(5)),
    DECL_GPIO_PIN(T931_GPIOE(6)),     DECL_GPIO_PIN(T931_GPIOE(7)),
};
#else
#define GPIO_PIN_COUNT 31
const fuchsia_hardware_pinimpl::Pin kGpioPins[] = {
    // For wifi.
    DECL_GPIO_PIN(T931_WIFI_HOST_WAKE),
    // For display.
    DECL_GPIO_PIN(GPIO_PANEL_DETECT),
    DECL_GPIO_PIN(GPIO_DDIC_DETECT),
    DECL_GPIO_PIN(GPIO_LCD_RESET),
    // For touch screen.
    DECL_GPIO_PIN(GPIO_TOUCH_INTERRUPT),
    DECL_GPIO_PIN(GPIO_TOUCH_RESET),
    // For audio out.
    DECL_GPIO_PIN(GPIO_AUDIO_SOC_FAULT_L),
    DECL_GPIO_PIN(GPIO_SOC_AUDIO_EN),
    // For Camera.
    DECL_GPIO_PIN(GPIO_VANA_ENABLE),
    DECL_GPIO_PIN(GPIO_VDIG_ENABLE),
    DECL_GPIO_PIN(GPIO_CAM_RESET),
    DECL_GPIO_PIN(GPIO_LIGHT_INTERRUPT),
    // For buttons.
    DECL_GPIO_PIN(GPIO_VOLUME_UP),
    DECL_GPIO_PIN(GPIO_VOLUME_DOWN),
    DECL_GPIO_PIN(GPIO_VOLUME_BOTH),
    DECL_GPIO_PIN(GPIO_MIC_PRIVACY),
    // For eMMC.
    DECL_GPIO_PIN(T931_EMMC_RST),
    // For SDIO.
    DECL_GPIO_PIN(T931_WIFI_REG_ON),
    // For OpenThread radio
    DECL_GPIO_PIN(GPIO_OT_RADIO_RESET),
    DECL_GPIO_PIN(GPIO_OT_RADIO_INTERRUPT),
    DECL_GPIO_PIN(GPIO_OT_RADIO_BOOTLOADER),
    // LED
    DECL_GPIO_PIN(GPIO_AMBER_LED),
    DECL_GPIO_PIN(GPIO_GREEN_LED),
    // For Bluetooth.
    DECL_GPIO_PIN(GPIO_SOC_WIFI_LPO_32k768),
    DECL_GPIO_PIN(GPIO_SOC_BT_REG_ON),

    // Board revision GPIOs.
    DECL_GPIO_PIN(GPIO_HW_ID0),
    DECL_GPIO_PIN(GPIO_HW_ID1),
};
#endif  // FACTORY_BUILD

// Add a separate device for GPIO C pins to allow the GPIO driver to be colocated with SPI and
// ot-radio. This eliminates two channel round-trips for each SPI transfer.
const fuchsia_hardware_pinimpl::Pin kGpioCPins[] = {
#ifdef FACTORY_BUILD
    DECL_GPIO_PIN(T931_GPIOC(0)), DECL_GPIO_PIN(T931_GPIOC(1)), DECL_GPIO_PIN(T931_GPIOC(2)),
    DECL_GPIO_PIN(T931_GPIOC(3)), DECL_GPIO_PIN(T931_GPIOC(4)), DECL_GPIO_PIN(T931_GPIOC(5)),
    DECL_GPIO_PIN(T931_GPIOC(6)), DECL_GPIO_PIN(T931_GPIOC(7)),
#else
    // For SPI interface.
    DECL_GPIO_PIN(GPIO_SPICC0_SS0),

    // Board revision GPIOs.
    DECL_GPIO_PIN(GPIO_HW_ID2),
    DECL_GPIO_PIN(GPIO_HW_ID3),
    DECL_GPIO_PIN(GPIO_HW_ID4),
#endif
};

static_assert(std::size(kGpioPins) + std::size(kGpioCPins) == GPIO_PIN_COUNT,
              "Incorrect pin count.");

zx_status_t CreateGpioCPlatformDevice(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  fuchsia_hardware_pinimpl::Metadata metadata{
      {.pins{{std::begin(kGpioCPins), std::end(kGpioCPins)}}}};

  fit::result persisted_metadata = fidl::Persist(metadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist pin metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  std::vector<fpbus::Metadata> gpio_c_metadata{
      {{
          .id = fuchsia_hardware_pinimpl::Metadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };

  fpbus::Node gpio_c_dev{{
      .name = "gpio-controller-c-ff634400",
      .vid = PDEV_VID_AMLOGIC,
      .pid = PDEV_PID_AMLOGIC_T931,
      .did = PDEV_DID_AMLOGIC_GPIO,
      .instance_id = 1,
      .mmio = kGpioMmios,
      .metadata = std::move(gpio_c_metadata),
  }};

  // TODO(https://fxbug.dev/42081248): Add the GPIO C device after all init steps have been executed
  // to ensure that there are no simultaneous accesses to these banks.
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('GPIO');
  auto composite_spec = fuchsia_driver_framework::wire::CompositeNodeSpec::Builder(fidl_arena)
                            .name("gpio-controller-c-ff634400")
                            .Build();

  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, gpio_c_dev),
                                                         composite_spec);
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

#ifdef GPIO_TEST
zx_status_t CreateTestGpioPlatformDevice(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  const std::vector<fuchsia_hardware_pinimpl::Pin> kTestGpioPins = {
      // Volume down, not used in this test.
      DECL_GPIO_PIN(T931_GPIOZ(5)),
      // Volume up, to test gpio_get_interrupt().
      DECL_GPIO_PIN(T931_GPIOZ(4)),
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

  fpbus::Node gpio_test_dev{{
      .name = "sherlock-gpio-test",
      .vid = PDEV_VID_GENERIC,
      .pid = PDEV_PID_GENERIC,
      .did = PDEV_DID_GPIO_TEST,
      .metadata = std::move(gpio_metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('GPIO');
  auto result = pbus.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, gpio_test_dev));
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

namespace sherlock {

zx_status_t Sherlock::CreateGpioPlatformDevice() {
  // The Focaltech touch driver expects the interrupt line to be driven by the touch controller.
  gpio_init_steps_.push_back(GpioPull(GPIO_TOUCH_INTERRUPT, fuchsia_hardware_pin::Pull::kNone));
  fuchsia_hardware_pinimpl::Metadata metadata{
      {.init_steps = std::move(gpio_init_steps_),
       .pins{{std::begin(kGpioPins), std::end(kGpioPins)}}}};
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
      .vid = PDEV_VID_AMLOGIC,
      .pid = PDEV_PID_AMLOGIC_T931,
      .did = PDEV_DID_AMLOGIC_GPIO,
      .instance_id = 0,
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
    zxlogf(ERROR, "Failed to add composite node spec: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

zx_status_t Sherlock::GpioInit() {
  if (zx_status_t status = CreateGpioPlatformDevice(); status != ZX_OK) {
    zxlogf(ERROR, "Failed to create gpio platform device: %s", zx_status_get_string(status));
    return status;
  }

  if (zx_status_t status = CreateGpioCPlatformDevice(pbus_); status != ZX_OK) {
    zxlogf(ERROR, "Failed to create gpio-c platform device: %s", zx_status_get_string(status));
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

}  // namespace sherlock
