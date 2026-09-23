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

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <soc/aml-common/aml-sdmmc.h>
#include <soc/aml-t931/t931-gpio.h>
#include <soc/aml-t931/t931-hw.h>

#include "sherlock.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

namespace {

static const std::vector<fpbus::Mmio> emmc_mmios{
    {{
        .base = T931_SD_EMMC_C_BASE,
        .length = T931_SD_EMMC_C_LENGTH,
    }},
};

static const std::vector<fpbus::Irq> emmc_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_SD_EMMC_C_IRQ),
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
    fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(T931_EMMC_RST)),
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

zx_status_t Sherlock::EmmcInit() {
  using fuchsia_hardware_pin::Pull;

  auto emmc_pin = [](uint32_t pin, fuchsia_hardware_pin::Pull pull) {
    return fuchsia_hardware_pinimpl::InitStep::WithCall({{
        .pin = pin,
        .call = fuchsia_hardware_pinimpl::InitCall::WithPinConfig({{
            .pull = pull,
            .function = T931_EMMC_D0_FN,
            .drive_strength_ua = 4'000,
        }}),
    }});
  };

  // set alternate functions to enable EMMC
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_D0, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_D1, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_D2, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_D3, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_D4, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_D5, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_D6, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_D7, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_CLK, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_RST, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_CMD, Pull::kUp));
  gpio_init_steps_.push_back(emmc_pin(T931_EMMC_DS, Pull::kDown));

  fidl::Arena<> fidl_arena;

  fit::result sdmmc_metadata = fidl::Persist(
      fuchsia_hardware_sdmmc::wire::SdmmcMetadata::Builder(fidl_arena)
          .max_frequency(166'666'667)
          // As per AMlogic, on S912 chipset, HS400 mode can be operated at 125MHZ or low.
          .speed_capabilities(fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs400)
          // Maintain the current Sherlock behavior until we determine that cache is needed.
          .enable_cache(false)
          // Maintain the current Sherlock behavior until we determine that eMMC Packed Commands are
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

  static const std::vector<fpbus::Metadata> sherlock_emmc_metadata{
      {{
          .id = fuchsia_hardware_sdmmc::wire::SdmmcMetadata::kSerializableName,
          .data = std::move(sdmmc_metadata.value()),
      }},
  };

  fpbus::Node emmc_dev;
  emmc_dev.name() = "mmc-ffe07000";
  emmc_dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
  emmc_dev.pid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC;
  emmc_dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_SDMMC_C;
  emmc_dev.mmio() = emmc_mmios;
  emmc_dev.irq() = emmc_irqs;
  emmc_dev.bti() = emmc_btis;
  emmc_dev.metadata() = sherlock_emmc_metadata;
  emmc_dev.boot_metadata() = emmc_boot_metadata;

  std::vector<fuchsia_driver_framework::ParentSpec2> kEmmcParents = {
      fuchsia_driver_framework::ParentSpec2{{kGpioResetRules, kGpioResetProperties}},
      fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProperties}}};

  fdf::Arena arena('EMMC');
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, emmc_dev),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "mmc-ffe07000", .parents2 = kEmmcParents}}));
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

}  // namespace sherlock
