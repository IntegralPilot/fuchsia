// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>

#include <sdk/lib/driver/component/cpp/composite_node_spec.h>
#include <sdk/lib/driver/component/cpp/node_add_args.h>
#include <soc/aml-s905d3/s905d3-gpio.h>
#include <soc/aml-s905d3/s905d3-hw.h>

#include "nelson-gpios.h"
#include "nelson.h"

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
        .base = S905D3_GPIO_BASE,
        .length = S905D3_GPIO_LENGTH,
    }},
    {{
        .base = S905D3_GPIO_AO_BASE,
        .length = S905D3_GPIO_AO_LENGTH,
    }},
    {{
        .base = S905D3_GPIO_INTERRUPT_BASE,
        .length = S905D3_GPIO_INTERRUPT_LENGTH,
    }},
};

const std::vector<fpbus::Irq> kGpioIrqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_GPIO_IRQ_0),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_GPIO_IRQ_1),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_GPIO_IRQ_2),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_GPIO_IRQ_3),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_GPIO_IRQ_4),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_GPIO_IRQ_5),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_GPIO_IRQ_6),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_GPIO_IRQ_7),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
};

zx_status_t CreateGpioHPlatformDevice(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  constexpr char kGpioHSchedulerRole[] = "fuchsia.devices.gpio.drivers.aml-gpio.gpioh";

  // The GPIO H device won't be able to provide interrupts for the pins it exposes, so
  // GPIO_SOC_SELINA_IRQ_OUT must be be exposed by the main GPIO device (see the list of pins above)
  // instead of this one.
  const std::vector<fuchsia_hardware_pinimpl::Pin> kGpioHPins = {
      DECL_GPIO_PIN(GPIO_SOC_SELINA_RESET), DECL_GPIO_PIN(GPIO_SOC_SPI_B_MOSI),
      DECL_GPIO_PIN(GPIO_SOC_SPI_B_MISO),   DECL_GPIO_PIN(GPIO_SOC_SPI_B_SS0),
      DECL_GPIO_PIN(GPIO_SOC_SPI_B_SCLK),   DECL_GPIO_PIN(GPIO_SOC_SELINA_OSC_EN)};

  const fuchsia_scheduler::RoleName kRoleName{kGpioHSchedulerRole};

  fit::result persisted_role_name = fidl::Persist(kRoleName);
  if (persisted_role_name.is_error()) {
    zxlogf(ERROR, "Failed to persist scheduler role name: %s",
           persisted_role_name.error_value().FormatDescription().c_str());
    return persisted_role_name.error_value().status();
  }

  fuchsia_hardware_pinimpl::Metadata pin_metadata{{.pins = kGpioHPins}};

  fit::result persisted_pin_metadata = fidl::Persist(pin_metadata);
  if (!persisted_pin_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist pin metadata: %s",
           persisted_pin_metadata.error_value().FormatDescription().c_str());
    return persisted_pin_metadata.error_value().status();
  }

  std::vector<fpbus::Metadata> gpio_h_metadata{
      {{
          .id = fuchsia_hardware_pinimpl::Metadata::kSerializableName,
          .data = std::move(persisted_pin_metadata.value()),
      }},
      {{
          .id = fuchsia_scheduler::RoleName::kSerializableName,
          .data = std::move(persisted_role_name.value()),
      }},
  };

  fpbus::Node gpio_h_dev{{
      .name = "gpio-h",
      .vid = PDEV_VID_AMLOGIC,
      .pid = PDEV_PID_AMLOGIC_S905D3,
      .did = PDEV_DID_AMLOGIC_GPIO,
      .instance_id = 1,
      .mmio = kGpioMmios,
      .metadata = std::move(gpio_h_metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('GPIO');

  auto composite_spec = fuchsia_driver_framework::wire::CompositeNodeSpec::Builder(fidl_arena)
                            .name("aml_gpio_h")
                            .Build();

  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, gpio_h_dev),
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

zx_status_t CreateGpioCPlatformDevice(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  const std::vector<fuchsia_hardware_pinimpl::Pin> kGpioCPins = {
      DECL_GPIO_PIN(GPIO_SOC_SPI_A_MISO),
      DECL_GPIO_PIN(GPIO_SOC_SPI_A_MOSI),
      DECL_GPIO_PIN(GPIO_SOC_SPI_A_SCLK),
      DECL_GPIO_PIN(GPIO_SOC_SPI_A_SS0),
  };

  const fuchsia_hardware_pinimpl::Metadata kMetadata{{.pins = kGpioCPins}};

  fit::result persisted_metadata = fidl::Persist(kMetadata);
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
      .name = "gpio-c",
      .vid = PDEV_VID_AMLOGIC,
      .pid = PDEV_PID_AMLOGIC_S905D3,
      .did = PDEV_DID_AMLOGIC_GPIO,
      .instance_id = 2,
      .mmio = kGpioMmios,
      .metadata = std::move(gpio_c_metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('GPIO');

  auto composite_spec = fuchsia_driver_framework::wire::CompositeNodeSpec::Builder(fidl_arena)
                            .name("aml_gpio_c")
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
  const std::vector<fuchsia_hardware_pinimpl::Pin> kTestGpioPins{
      // SYS_LED
      DECL_GPIO_PIN(GPIO_AMBER_LED_PWM),
      // JTAG Adapter Pin
      DECL_GPIO_PIN(GPIO_SOC_JTAG_TCK),
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
      .vid = PDEV_VID_GENERIC,
      .pid = PDEV_PID_GENERIC,
      .did = PDEV_DID_GPIO_TEST,
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

namespace nelson {

zx_status_t Nelson::CreateGpioPlatformDevice() {
  // GPIOs to expose from generic GPIO driver. Do not expose C bank or H bank GPIOs here, as they
  // are managed by separate devices below. The three GPIO devices are not capable of synchronizing
  // accesses to the interrupt registers, so C and H bank GPIOs that are used for interrupts must be
  // exposed by the main device (only GPIO_SOC_SELINA_IRQ_OUT and GPIO_TH_SOC_INT). All pins can be
  // used in calls from the board driver, regardless of bank.
  const std::vector<fuchsia_hardware_pinimpl::Pin> kGpioPins = {
      DECL_GPIO_PIN(GPIO_INRUSH_EN_SOC),
      DECL_GPIO_PIN(GPIO_SOC_I2S_SCLK),
      DECL_GPIO_PIN(GPIO_SOC_I2S_FS),
      DECL_GPIO_PIN(GPIO_SOC_I2S_DO0),
      DECL_GPIO_PIN(GPIO_SOC_I2S_DIN0),
      DECL_GPIO_PIN(GPIO_SOC_AUDIO_EN),
      DECL_GPIO_PIN(GPIO_SOC_MIC_DCLK),
      DECL_GPIO_PIN(GPIO_SOC_MICLR_DIN0),
      DECL_GPIO_PIN(GPIO_SOC_MICLR_DIN1),
      DECL_GPIO_PIN(GPIO_SOC_BKL_EN),
      DECL_GPIO_PIN(GPIO_AUDIO_SOC_FAULT_L),
      DECL_GPIO_PIN(GPIO_SOC_TH_RST_L),
      DECL_GPIO_PIN(GPIO_SOC_AV_I2C_SDA),
      DECL_GPIO_PIN(GPIO_SOC_AV_I2C_SCL),
      DECL_GPIO_PIN(GPIO_HW_ID_3),
      DECL_GPIO_PIN(GPIO_SOC_TH_BOOT_MODE_L),
      DECL_GPIO_PIN(GPIO_MUTE_SOC),
      DECL_GPIO_PIN(GPIO_HW_ID_2),
      DECL_GPIO_PIN(GPIO_TOUCH_SOC_INT_L),
      DECL_GPIO_PIN(GPIO_VOL_UP_L),
      DECL_GPIO_PIN(GPIO_VOL_DN_L),
      DECL_GPIO_PIN(GPIO_HW_ID_0),
      DECL_GPIO_PIN(GPIO_HW_ID_1),
      DECL_GPIO_PIN(GPIO_SOC_TOUCH_RST_L),
      DECL_GPIO_PIN(GPIO_ALERT_PWR_L),
      DECL_GPIO_PIN(GPIO_DISP_SOC_ID0),
      DECL_GPIO_PIN(GPIO_DISP_SOC_ID1),
      DECL_GPIO_PIN(GPIO_SOC_DISP_RST_L),
      DECL_GPIO_PIN(GPIO_SOC_TOUCH_I2C_SDA),
      DECL_GPIO_PIN(GPIO_SOC_TOUCH_I2C_SCL),
      // ot-radio is responsible for not making concurrent calls to this GPIO and the GPIO C device
      // (or other clients of that device, namely SPI0). Calls may be made on the interrupt object
      // (and interrupts may be received) at any time, as there is no GPIO driver involvement in
      // that case.
      DECL_GPIO_PIN(GPIO_TH_SOC_INT),
      DECL_GPIO_PIN(GPIO_SOC_TH_INT),
      DECL_GPIO_PIN(GPIO_SOC_WIFI_SDIO_D0),
      DECL_GPIO_PIN(GPIO_SOC_WIFI_SDIO_D1),
      DECL_GPIO_PIN(GPIO_SOC_WIFI_SDIO_D2),
      DECL_GPIO_PIN(GPIO_SOC_WIFI_SDIO_D3),
      DECL_GPIO_PIN(GPIO_SOC_WIFI_SDIO_CLK),
      DECL_GPIO_PIN(GPIO_SOC_WIFI_SDIO_CMD),
      DECL_GPIO_PIN(GPIO_SOC_WIFI_REG_ON),
      DECL_GPIO_PIN(GPIO_WIFI_SOC_WAKE),
      DECL_GPIO_PIN(GPIO_SOC_BT_PCM_IN),
      DECL_GPIO_PIN(GPIO_SOC_BT_PCM_OUT),
      DECL_GPIO_PIN(GPIO_SOC_BT_PCM_SYNC),
      DECL_GPIO_PIN(GPIO_SOC_BT_PCM_CLK),
      DECL_GPIO_PIN(GPIO_SOC_BT_UART_TX),
      DECL_GPIO_PIN(GPIO_SOC_BT_UART_RX),
      DECL_GPIO_PIN(GPIO_SOC_BT_UART_CTS),
      DECL_GPIO_PIN(GPIO_SOC_BT_UART_RTS),
      DECL_GPIO_PIN(GPIO_SOC_WIFI_LPO_32K768),
      DECL_GPIO_PIN(GPIO_SOC_BT_REG_ON),
      DECL_GPIO_PIN(GPIO_BT_SOC_WAKE),
      DECL_GPIO_PIN(GPIO_SOC_BT_WAKE),
      // Like above -- Selina is responsible for not making current calls to this GPIO and the GPIO
      // H device (or SPI1, which is a client of the GPIO H device).
      DECL_GPIO_PIN(GPIO_SOC_SELINA_IRQ_OUT),
      DECL_GPIO_PIN(GPIO_SOC_DEBUG_UARTAO_TX),
      DECL_GPIO_PIN(GPIO_SOC_DEBUG_UARTAO_RX),
      DECL_GPIO_PIN(GPIO_SOC_SENSORS_I2C_SCL),
      DECL_GPIO_PIN(GPIO_SOC_SENSORS_I2C_SDA),
      DECL_GPIO_PIN(GPIO_HW_ID_4),
      DECL_GPIO_PIN(GPIO_RGB_SOC_INT_L),
      DECL_GPIO_PIN(GPIO_SOC_JTAG_TCK),
      DECL_GPIO_PIN(GPIO_SOC_JTAG_TMS),
      DECL_GPIO_PIN(GPIO_SOC_JTAG_TDI),
      DECL_GPIO_PIN(GPIO_SOC_JTAG_TDO),
      DECL_GPIO_PIN(GPIO_FDR_L),
      DECL_GPIO_PIN(GPIO_AMBER_LED_PWM),
      DECL_GPIO_PIN(GPIO_SOC_VDDEE_PWM),
      DECL_GPIO_PIN(GPIO_SOC_VDDCPU_PWM),
      DECL_GPIO_PIN(SOC_EMMC_D0),
      DECL_GPIO_PIN(SOC_EMMC_D1),
      DECL_GPIO_PIN(SOC_EMMC_D2),
      DECL_GPIO_PIN(SOC_EMMC_D3),
      DECL_GPIO_PIN(SOC_EMMC_D4),
      DECL_GPIO_PIN(SOC_EMMC_D5),
      DECL_GPIO_PIN(SOC_EMMC_D6),
      DECL_GPIO_PIN(SOC_EMMC_D7),
      DECL_GPIO_PIN(SOC_EMMC_CLK),
      DECL_GPIO_PIN(SOC_EMMC_CMD),
      DECL_GPIO_PIN(SOC_EMMC_RST_L),
      DECL_GPIO_PIN(SOC_EMMC_DS),
  };

  // Enable mute LED so it will be controlled by mute switch.
  gpio_init_steps_.push_back(GpioOutput(GPIO_AMBER_LED_PWM, true));
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

  fpbus::Node gpio_dev{{.name = "gpio",
                        .vid = PDEV_VID_AMLOGIC,
                        .pid = PDEV_PID_AMLOGIC_S905D3,
                        .did = PDEV_DID_AMLOGIC_GPIO,
                        .mmio = kGpioMmios,
                        .irq = kGpioIrqs,
                        .metadata = std::move(gpio_metadata)}};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('GPIO');

  auto composite_spec = fuchsia_driver_framework::wire::CompositeNodeSpec::Builder(fidl_arena)
                            .name("aml_gpio")
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

zx_status_t Nelson::GpioInit() {
  if (zx_status_t status = CreateGpioPlatformDevice(); status != ZX_OK) {
    zxlogf(ERROR, "Failed to create gpio platform device: %s", zx_status_get_string(status));
    return status;
  }

  if (zx_status_t status = CreateGpioHPlatformDevice(pbus_); status != ZX_OK) {
    zxlogf(ERROR, "Failed to create gpio-h platform device: %s", zx_status_get_string(status));
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

}  // namespace nelson
