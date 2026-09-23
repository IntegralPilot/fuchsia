// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.sdmmc/cpp/wire.h>
#include <fuchsia/hardware/sdmmc/c/banjo.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zx/handle.h>
#include <zircon/hw/gpt.h>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <soc/aml-common/aml-sdmmc.h>
#include <soc/aml-s905d3/s905d3-gpio.h>
#include <soc/aml-s905d3/s905d3-hw.h>

#include "nelson-gpios.h"
#include "nelson.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;

namespace {

static const std::vector<fpbus::Mmio> emmc_mmios{
    {{
        .base = S905D3_EMMC_C_SDIO_BASE,
        .length = S905D3_EMMC_C_SDIO_LENGTH,
    }},
};

static const std::vector<fpbus::Irq> emmc_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_EMMC_C_SDIO_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

static const std::vector<fpbus::Bti> emmc_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_EMMC,
    }},
};

static const std::vector<fpbus::BootMetadata> emmc_boot_metadata{
    {{
        .zbi_type = ZBI_TYPE_DRV_PARTITION_MAP,
        .zbi_extra = 0,
    }},
};

const std::vector<fuchsia_driver_framework::BindRule2> kGpioResetRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(SOC_EMMC_RST_L)),
};

const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioResetProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-reset"),
};

const std::vector<fuchsia_driver_framework::BindRule2> kGpioInitRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioInitProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

}  // namespace

zx_status_t Nelson::EmmcInit() {
  // set alternate functions to enable EMMC
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_D0, S905D3_EMMC_D0_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_D1, S905D3_EMMC_D1_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_D2, S905D3_EMMC_D2_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_D3, S905D3_EMMC_D3_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_D4, S905D3_EMMC_D4_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_D5, S905D3_EMMC_D5_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_D6, S905D3_EMMC_D6_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_D7, S905D3_EMMC_D7_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_CLK, S905D3_EMMC_CLK_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_RST, S905D3_EMMC_RST_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_CMD, S905D3_EMMC_CMD_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D3_EMMC_DS, S905D3_EMMC_DS_FN));

  fidl::Arena<> fidl_arena;

  fit::result sdmmc_metadata = fidl::Persist(
      fuchsia_hardware_sdmmc::wire::SdmmcMetadata::Builder(fidl_arena)
          .max_frequency(166'666'667)
          .speed_capabilities(fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs400)
          // Maintain the current Nelson behavior until we determine that cache is needed.
          .enable_cache(false)
          // Maintain the current Nelson behavior until we determine that eMMC Packed Commands are
          // needed.
          .max_command_packing(0)
          // TODO(https://fxbug.dev/42084501): Use the FIDL SDMMC protocol.
          .use_fidl(false)
          .Build());
  if (!sdmmc_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to encode SDMMC metadata: %s",
           sdmmc_metadata.error_value().FormatDescription().c_str());
    return sdmmc_metadata.error_value().status();
  }

  static const std::vector<fpbus::Metadata> emmc_metadata{
      {{
          .id = fuchsia_hardware_sdmmc::wire::SdmmcMetadata::kSerializableName,
          .data = std::move(sdmmc_metadata.value()),
      }},
  };

  static const fpbus::Node emmc_dev = []() {
    fpbus::Node dev = {};
    dev.name() = "nelson-emmc";
    dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
    dev.pid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC;
    dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_SDMMC_C;
    dev.mmio() = emmc_mmios;
    dev.irq() = emmc_irqs;
    dev.bti() = emmc_btis;
    dev.metadata() = emmc_metadata;
    dev.boot_metadata() = emmc_boot_metadata;
    return dev;
  }();

  std::vector<fuchsia_driver_framework::ParentSpec2> kEmmcParents = {
      fuchsia_driver_framework::ParentSpec2{{kGpioResetRules, kGpioResetProperties}},
      fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProperties}}};

  fdf::Arena arena('EMMC');
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, emmc_dev),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "nelson_emmc", .parents2 = kEmmcParents}}));
  if (!result.ok()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Emmc(emmc_dev) request failed: %s",
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Emmc(emmc_dev) failed: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace nelson
