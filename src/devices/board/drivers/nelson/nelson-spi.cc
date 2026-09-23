// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.amlogic.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spi.businfo/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zx/time.h>

#include <optional>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <fbl/algorithm.h>
#include <soc/aml-common/aml-registers.h>
#include <soc/aml-s905d2/s905d2-gpio.h>
#include <soc/aml-s905d3/s905d3-hw.h>

#include "nelson-gpios.h"
#include "nelson.h"
#include "src/devices/lib/fidl-metadata/spi.h"

#define HHI_SPICC_CLK_CNTL (0xf7 * 4)

#define spicc0_clk_sel_fclk_div4 (2 << 7)
#define spicc0_clk_en (1 << 6)
#define spicc0_clk_div(x) ((x) - 1)

#define spicc1_clk_sel_fclk_div3 (3 << 23)
#define spicc1_clk_en (1 << 22)
#define spicc1_clk_div(x) (((x) - 1) << 16)

namespace {

constexpr char kSpi1SchedulerRole[] = "fuchsia.devices.spi.drivers.aml-spi.spi1";

fuchsia_hardware_pinimpl::InitStep SpiPin(
    uint32_t pin, uint64_t function,
    std::optional<fuchsia_hardware_pin::Pull> pull = std::nullopt) {
  return fuchsia_hardware_pinimpl::InitStep::WithCall({{
      .pin = pin,
      .call = fuchsia_hardware_pinimpl::InitCall::WithPinConfig({{
          .pull = pull,
          .function = function,
          .drive_strength_ua = 2'500,
      }}),
  }});
}

}  // namespace

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;
using spi_channel_t = fidl_metadata::spi::Channel;

fdf::wire::CompositeNodeSpec MakeSpiCompositeNodeSpec(fidl::AnyArena& fidl_arena, std::string name,
                                                      uint32_t gpio_pin, std::string gpio_name,
                                                      std::string register_id) {
  const std::vector kGpioSpiRules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID, gpio_pin),
  };

  const std::vector kGpioSpiProperties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, gpio_name),
  };

  const std::vector kResetRegisterRules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::NAME, register_id),
  };

  const std::vector kResetRegisterProperties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "reset"),
  };

  const std::vector<fuchsia_driver_framework::BindRule2> kGpioInitRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };
  const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioInitProperties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
  };

  const std::vector<fuchsia_driver_framework::ParentSpec2> parents = {
      {kGpioSpiRules, kGpioSpiProperties},
      {kResetRegisterRules, kResetRegisterProperties},
      {kGpioInitRules, kGpioInitProperties},
  };

  return fidl::ToWire(
      fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{{.name = name, .parents2 = parents}});
}

zx_status_t Nelson::SpiInit() {
  constexpr uint32_t kSpiccClkValue =
      // SPICC0 clock enable (500 MHz)
      spicc0_clk_sel_fclk_div4 | spicc0_clk_en | spicc0_clk_div(1) |

      // SPICC1 clock enable (666 MHz)
      spicc1_clk_sel_fclk_div3 | spicc1_clk_en | spicc1_clk_div(1);

  // TODO(https://fxbug.dev/42109271): fix this clock enable block when the clock driver can handle
  // the dividers
  {
    zx::unowned_resource resource(get_mmio_resource(parent()));
    const size_t rounded_size = fbl::round_up<size_t>(S905D3_HIU_LENGTH, zx_system_get_page_size());
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create_physical(*resource, S905D3_HIU_BASE, rounded_size, &vmo);
    if (status != ZX_OK) {
      zxlogf(ERROR, "failed to create VMO: %s", zx_status_get_string(status));
      return status;
    }
    zx::result<fdf::MmioBuffer> buf = fdf::MmioBuffer::Create(0, S905D3_HIU_LENGTH, std::move(vmo),
                                                              ZX_CACHE_POLICY_UNCACHED_DEVICE);
    if (buf.is_error()) {
      zxlogf(ERROR, "fdf::MmioBuffer::Create() error: %s", buf.status_string());
      return buf.status_value();
    }

    buf->Write32(kSpiccClkValue, HHI_SPICC_CLK_CNTL);
  }

  zx_status_t status0 = Spi0Init();
  zx_status_t status1 = Spi1Init();
  return status0 == ZX_OK ? status1 : status0;
}

zx_status_t Nelson::Spi0Init() {
  static const std::vector<fpbus::Mmio> spi_0_mmios{
      {{
          .base = S905D3_SPICC0_BASE,
          .length = S905D3_SPICC0_LENGTH,
      }},
  };

  static const std::vector<fpbus::Irq> spi_0_irqs{
      {{
          .irq = fpbus::IrqSpec::WithIrq(S905D3_SPICC0_IRQ),
          .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
      }},
  };

  static const spi_channel_t spi_0_channels[] = {
      {
          .cs = 0,  // index into matching chip-select map
          .vid = PDEV_VID_NORDIC,
          .pid = PDEV_PID_NORDIC_NRF52811,
          .did = PDEV_DID_NORDIC_THREAD,
      },
  };

  static const fuchsia_hardware_amlogic_metadata::SpiConfig kSpi0Config({
      .bus_id = NELSON_SPICC0,
      .cs = {0},                                       // index into fragments list
      .clock_divider_register_value = (500 >> 1) - 1,  // SCLK = core clock / 500 = 1.0 MHz
      .use_enhanced_clock_mode = true,
      .delay_control = fuchsia_hardware_amlogic_metadata::kDefaultDelayControl,
  });

  fpbus::Node spi_0_dev;
  spi_0_dev.name() = "spi-0";
  spi_0_dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
  spi_0_dev.pid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC;
  spi_0_dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_SPI;
  spi_0_dev.instance_id() = 0;
  spi_0_dev.mmio() = spi_0_mmios;
  spi_0_dev.irq() = spi_0_irqs;

  gpio_init_steps_.push_back(SpiPin(GPIO_SOC_SPI_A_MOSI, 5));  // MOSI
                                                               //
  gpio_init_steps_.push_back(SpiPin(GPIO_SOC_SPI_A_MISO, 5));  // MISO

  gpio_init_steps_.push_back(GpioFunction(GPIO_SOC_SPI_A_SS0, 0));
  gpio_init_steps_.push_back(GpioOutput(GPIO_SOC_SPI_A_SS0, true));  // SS0

  // SCLK must be pulled down to prevent SPI bit errors.
  gpio_init_steps_.push_back(
      SpiPin(GPIO_SOC_SPI_A_SCLK, 5, fuchsia_hardware_pin::Pull::kDown));  // SCLK

  std::vector<uint8_t> persisted_spi_bus_metadata;
  {
    zx::result result = fidl_metadata::spi::SpiChannelsToFidl(NELSON_SPICC0, spi_0_channels);
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to convert spi channels to fidl: %s", result.status_string());
      return result.error_value();
    }
    persisted_spi_bus_metadata = std::move(result.value());
  }

  fit::result persisted_config = fidl::Persist(kSpi0Config);
  if (!persisted_config.is_ok()) {
    zxlogf(ERROR, "Failed to persist spi 0 config: %s",
           persisted_config.error_value().FormatDescription().c_str());
    return persisted_config.error_value().status();
  }

  spi_0_dev.metadata() = std::vector<fpbus::Metadata>{
      {{.id = fuchsia_hardware_amlogic_metadata::SpiConfig::kSerializableName,
        .data = std::move(persisted_config.value())}},
      {{.id = fuchsia_hardware_spi_businfo::SpiBusMetadata::kSerializableName,
        .data = std::move(persisted_spi_bus_metadata)}},
  };

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('SPI0');
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, spi_0_dev),
      MakeSpiCompositeNodeSpec(
          fidl_arena, "spi_0", /* gpio_pin */ GPIO_SOC_SPI_A_SS0,
          /* gpio_name */ "gpio-cs-0",
          /* register_id */ bind_fuchsia_amlogic_platform::NAME_REGISTER_SPICC0_RESET));
  if (!result.ok()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Spi0(spi_0_dev) request failed: %s",
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Spi0(spi_0_dev) failed: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

zx_status_t Nelson::Spi1Init() {
  static const std::vector<fpbus::Mmio> spi_1_mmios{
      {{
          .base = S905D3_SPICC1_BASE,
          .length = S905D3_SPICC1_LENGTH,
      }},
  };

  static const std::vector<fpbus::Irq> spi_1_irqs{
      {{
          .irq = fpbus::IrqSpec::WithIrq(S905D3_SPICC1_IRQ),
          .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
      }},
  };

  static const std::vector<fpbus::Bti> spi_1_btis{
      {{
          .iommu_id = 0,
          .bti_id = BTI_SPI1,
      }},
  };

  static const spi_channel_t spi_1_channels[] = {
      // Radar sensor head.
      {
          .cs = 0,  // index into matching chip-select map
          .vid = PDEV_VID_INFINEON,
          .pid = PDEV_PID_INFINEON_BGT60TR13C,
          .did = PDEV_DID_RADAR_SENSOR,
      },
  };

  constexpr uint32_t kMoNoDelay = 0 << 0;
  constexpr uint32_t kMiDelay3Cycles = 3 << 2;
  constexpr uint32_t kMiCapAhead2Cycles = 0 << 4;

  static const fuchsia_hardware_amlogic_metadata::SpiConfig kSpi1Config({
      .bus_id = NELSON_SPICC1,
      .cs = {fuchsia_hardware_amlogic_metadata::kCsClientManaged},  // CS GPIO managed by client
                                                                    // driver
      .clock_divider_register_value = (22 >> 1) - 1,  // SCLK = core clock / 22 = 30.3 MHz
      .use_enhanced_clock_mode = true,
      .client_reverses_dma_transfers = true,
      .delay_control = kMoNoDelay | kMiDelay3Cycles | kMiCapAhead2Cycles,
  });

  fpbus::Node spi_1_dev;
  spi_1_dev.name() = "spi-1";
  spi_1_dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
  spi_1_dev.pid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC;
  spi_1_dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_SPI;
  spi_1_dev.instance_id() = 1;
  spi_1_dev.mmio() = spi_1_mmios;
  spi_1_dev.irq() = spi_1_irqs;
  spi_1_dev.bti() = spi_1_btis;

  // setup pinmux for SPICC1 bus arbiter.
  gpio_init_steps_.push_back(SpiPin(GPIO_SOC_SPI_B_MOSI, 3));  // MOSI

  gpio_init_steps_.push_back(SpiPin(GPIO_SOC_SPI_B_MISO, 3));  // MISO

  gpio_init_steps_.push_back(GpioOutput(GPIO_SOC_SPI_B_SS0, true));  // SS0

  gpio_init_steps_.push_back(SpiPin(GPIO_SOC_SPI_B_SCLK, 3));  // SCLK

  fit::result persisted_config = fidl::Persist(kSpi1Config);
  if (!persisted_config.is_ok()) {
    zxlogf(ERROR, "Failed to persist spi 1 config: %s",
           persisted_config.error_value().FormatDescription().c_str());
    return persisted_config.error_value().status();
  }

  std::vector<fpbus::Metadata> spi_1_metadata;
  spi_1_metadata.emplace_back(
      fpbus::Metadata({.id = fuchsia_hardware_amlogic_metadata::SpiConfig::kSerializableName,
                       .data = std::move(persisted_config.value())}));

  {
    const fuchsia_scheduler::RoleName kRoleName(kSpi1SchedulerRole);

    fit::result persisted = fidl::Persist(kRoleName);
    if (persisted.is_error()) {
      zxlogf(ERROR, "Failed to persist scheduler role name: %s",
             persisted.error_value().FormatDescription().c_str());
      return persisted.error_value().status();
    }

    spi_1_metadata.emplace_back(fpbus::Metadata{{
        .id = fuchsia_scheduler::RoleName::kSerializableName,
        .data = std::move(persisted.value()),
    }});
  }

  std::vector<uint8_t> persisted_spi_bus_metadata;
  {
    zx::result result = fidl_metadata::spi::SpiChannelsToFidl(NELSON_SPICC1, spi_1_channels);
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to convert spi channels to fidl: %s", result.status_string());
      return result.error_value();
    }
    persisted_spi_bus_metadata = std::move(result.value());
  }

  spi_1_metadata.emplace_back(
      fpbus::Metadata{{.id = fuchsia_hardware_spi_businfo::SpiBusMetadata::kSerializableName,
                       .data = std::move(persisted_spi_bus_metadata)}});

  spi_1_dev.metadata() = std::move(spi_1_metadata);

  fdf::Arena arena('SPI1');
  fidl::Arena<> fidl_arena;
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, spi_1_dev),
      MakeSpiCompositeNodeSpec(
          fidl_arena, "spi_1", /* gpio_pin */ GPIO_SOC_SPI_B_SS0,
          /* gpio_name */ "gpio-cs-0",
          /* register_id */ bind_fuchsia_amlogic_platform::NAME_REGISTER_SPICC1_RESET));
  if (!result.ok()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Spi1(spi_1_dev) request failed: %s",
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Spi1(spi_1_dev) failed: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace nelson
