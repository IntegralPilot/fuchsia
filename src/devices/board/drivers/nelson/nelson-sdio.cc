// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.sdmmc/cpp/fidl.h>
#include <fidl/fuchsia.wlan.broadcom/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zircon-internal/align.h>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/broadcom/platform/cpp/bind.h>
#include <bind/fuchsia/broadcom/platform/sdio/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <bind/fuchsia/pwm/cpp/bind.h>
#include <bind/fuchsia/sdio/cpp/bind.h>
#include <fbl/algorithm.h>
#include <hwreg/bitfields.h>
#include <soc/aml-common/aml-sdmmc.h>
#include <soc/aml-s905d3/s905d3-gpio.h>
#include <soc/aml-s905d3/s905d3-hw.h>

#include "nelson-gpios.h"
#include "nelson.h"
#include "src/devices/lib/broadcom/commands.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::BootMetadata> wifi_boot_metadata{
    {{
        .zbi_type = ZBI_TYPE_DRV_MAC_ADDRESS,
        .zbi_extra = MACADDR_WIFI,
    }},
};

static const std::vector<fpbus::Mmio> sd_emmc_mmios{
    {{
        .base = S905D3_EMMC_A_SDIO_BASE,
        .length = S905D3_EMMC_A_SDIO_LENGTH,
    }},
    {{
        .base = S905D3_GPIO_BASE,
        .length = S905D3_GPIO_LENGTH,
    }},
    {{
        .base = S905D3_HIU_BASE,
        .length = S905D3_HIU_LENGTH,
    }},
};

static const std::vector<fpbus::Irq> sd_emmc_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_EMMC_A_SDIO_IRQ),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
};

static const std::vector<fpbus::Bti> sd_emmc_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_SDIO,
    }},
};

// Composite binding rules for wifi driver.

// Composite node specs for SDIO.
const std::vector<fuchsia_driver_framework::BindRule2> kPwmRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM),
};

const std::vector<fuchsia_driver_framework::NodeProperty2> kPwmProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM),
};

const std::vector<fuchsia_driver_framework::BindRule2> kGpioResetRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(GPIO_SOC_WIFI_REG_ON)),
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

zx::result<> AddWifiNode(fdf::WireSyncClient<fpbus::PlatformBus>& pbus) {
  static const fuchsia_wlan_broadcom::WifiConfig kWifiConfig{{
      .oob_irq_mode = ZX_INTERRUPT_MODE_LEVEL_HIGH,
      .clm_needed = true,
      .iovar_table =
          {
              fuchsia_wlan_broadcom::IovarEntry::WithString(
                  {{.name = "ampdu_ba_wsize", .val = 32}}),
              fuchsia_wlan_broadcom::IovarEntry::WithString(
                  {{.name = "stbc_tx", .val = 0}}),  // since tx_streams is 1
              fuchsia_wlan_broadcom::IovarEntry::WithString({{.name = "stbc_rx", .val = 1}}),
              fuchsia_wlan_broadcom::IovarEntry::WithCommand({{.cmd = BRCMF_C_SET_PM, .val = 0}}),
              fuchsia_wlan_broadcom::IovarEntry::WithCommand(
                  {{.cmd = BRCMF_C_SET_FAKEFRAG, .val = 1}}),
          },
      .cc_table =
          {
              {"WW", 2},   {"AU", 924}, {"CA", 902}, {"US", 844}, {"GB", 890}, {"BE", 890},
              {"BG", 890}, {"CZ", 890}, {"DK", 890}, {"DE", 890}, {"EE", 890}, {"IE", 890},
              {"GR", 890}, {"ES", 890}, {"FR", 890}, {"HR", 890}, {"IT", 890}, {"CY", 890},
              {"LV", 890}, {"LT", 890}, {"LU", 890}, {"HU", 890}, {"MT", 890}, {"NL", 890},
              {"AT", 890}, {"PL", 890}, {"PT", 890}, {"RO", 890}, {"SI", 890}, {"SK", 890},
              {"FI", 890}, {"SE", 890}, {"EL", 890}, {"IS", 890}, {"LI", 890}, {"TR", 890},
              {"CH", 890}, {"NO", 890}, {"JP", 3},   {"KR", 3},   {"TW", 3},   {"IN", 3},
              {"SG", 3},   {"MX", 3},   {"CL", 3},   {"PE", 3},   {"CO", 3},   {"NZ", 3},
              {"", 0},
          },
  }};

  static const std::vector<fuchsia_driver_framework::BindRule2> kGpioWifiHostRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(S905D3_WIFI_SDIO_WAKE_HOST)),
  };

  static const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioWifiHostProperties =
      std::vector{
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
          fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-oob"),
  };

  fit::result persisted_wifi_config = fidl::Persist(kWifiConfig);
  if (!persisted_wifi_config.is_ok()) {
    zxlogf(ERROR, "Failed to persist wifi config: %s",
           persisted_wifi_config.error_value().FormatDescription().c_str());
    return zx::error(persisted_wifi_config.error_value().status());
  }

  std::vector<fpbus::Metadata> metadata{
      {{
          .id = fuchsia_wlan_broadcom::WifiConfig::kSerializableName,
          .data = std::move(persisted_wifi_config.value()),
      }},
  };

  fpbus::Node node{{
      .name = "wifi",
      .vid = bind_fuchsia_broadcom_platform::BIND_PLATFORM_DEV_VID_BROADCOM,
      .pid = bind_fuchsia_broadcom_platform::BIND_PLATFORM_DEV_PID_BCM43458,
      .did = bind_fuchsia_broadcom_platform::BIND_PLATFORM_DEV_DID_WIFI,
      .metadata = std::move(metadata),
      .boot_metadata = wifi_boot_metadata,
  }};

  constexpr uint32_t kSdioFunctionCount = 2;
  std::vector<fuchsia_driver_framework::ParentSpec2> wifi_parents = {
      fuchsia_driver_framework::ParentSpec2{{kGpioWifiHostRules, kGpioWifiHostProperties}},
      fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProperties}},
  };
  wifi_parents.reserve(wifi_parents.size() + kSdioFunctionCount);
  for (uint32_t i = 1; i <= kSdioFunctionCount; i++) {
    auto sdio_bind_rules = {
        fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_sdio::BIND_PROTOCOL_DEVICE),
        fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.sdio.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia::SDIO_VID,
                                bind_fuchsia_broadcom_platform_sdio::BIND_SDIO_VID_BROADCOM),
        fdf::MakeAcceptBindRule(bind_fuchsia::SDIO_PID,
                                bind_fuchsia_broadcom_platform_sdio::BIND_SDIO_PID_BCM4345),
        fdf::MakeAcceptBindRule(bind_fuchsia::SDIO_FUNCTION, i),
    };

    auto sdio_properties = {
        fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.sdio.Service"),
        fdf::MakeProperty2(bind_fuchsia::SDIO_FUNCTION, i),
    };

    wifi_parents.push_back(fuchsia_driver_framework::ParentSpec2{
        {sdio_bind_rules, sdio_properties},
    });
  }

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('WIFI');
  fdf::WireUnownedResult result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, node),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "wifi", .parents2 = wifi_parents}}));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request: %s",
           result.FormatDescription().data());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add composite node spec: %s",
           zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }

  return zx::ok();
}

zx::result<> AddSdEmmcNode(fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  static const fuchsia_hardware_sdmmc::SdmmcMetadata kMetadata{{
      .max_frequency = 208'000'000,
      // TODO(https://fxbug.dev/42084501): Use the FIDL SDMMC protocol.
      .use_fidl = false,
  }};

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist SDMMC metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return zx::error(persisted_metadata.error_value().status());
  }

  std::vector<fpbus::Metadata> metadata{
      {{
          .id = fuchsia_hardware_sdmmc::SdmmcMetadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };

  fpbus::Node sd_emmc_dev{{
      .name = "aml-sdio",
      .vid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC,
      .pid = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC,
      .did = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_SDMMC_A,
      .mmio = sd_emmc_mmios,
      .irq = sd_emmc_irqs,
      .bti = sd_emmc_btis,
      .metadata = std::move(metadata),
  }};

  std::vector<fuchsia_driver_framework::ParentSpec2> kSdioParents = {
      fuchsia_driver_framework::ParentSpec2{{kPwmRules, kPwmProperties}},
      fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProperties}},
      fuchsia_driver_framework::ParentSpec2{{kGpioResetRules, kGpioResetProperties}}};

  fidl::Arena<> fidl_arena;
  fdf::Arena sdio_arena('SDIO');
  auto result =
      pbus.buffer(sdio_arena)
          ->AddCompositeNodeSpec(
              fidl::ToWire(fidl_arena, sd_emmc_dev),
              fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                           {.name = "aml_sdio", .parents2 = kSdioParents}}));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request: %s",
           result.FormatDescription().data());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add composite node spec: %s",
           zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }

  return zx::ok();
}

zx_status_t Nelson::SdioInit() {
  auto sdio_pin = [](uint32_t pin) {
    return fuchsia_hardware_pinimpl::InitStep::WithCall({{
        .pin = pin,
        .call = fuchsia_hardware_pinimpl::InitCall::WithPinConfig({{
            .function = S905D3_WIFI_SDIO_D0_FN,
            .drive_strength_ua = 4'000,
        }}),
    }});
  };

  gpio_init_steps_.push_back(sdio_pin(S905D3_WIFI_SDIO_D0));
  gpio_init_steps_.push_back(sdio_pin(S905D3_WIFI_SDIO_D1));
  gpio_init_steps_.push_back(sdio_pin(S905D3_WIFI_SDIO_D2));
  gpio_init_steps_.push_back(sdio_pin(S905D3_WIFI_SDIO_D3));
  gpio_init_steps_.push_back(sdio_pin(S905D3_WIFI_SDIO_CLK));
  gpio_init_steps_.push_back(sdio_pin(S905D3_WIFI_SDIO_CMD));
  gpio_init_steps_.push_back(GpioFunction(S905D3_WIFI_SDIO_WAKE_HOST, 0));

  gpio_init_steps_.push_back(
      GpioPull(S905D3_WIFI_SDIO_WAKE_HOST, fuchsia_hardware_pin::Pull::kNone));

  if (zx::result result = AddSdEmmcNode(pbus_); result.is_error()) {
    zxlogf(ERROR, "Failed to add sd-emmc node: %s", result.status_string());
    return result.status_value();
  }

  if (zx::result result = AddWifiNode(pbus_); result.is_error()) {
    zxlogf(ERROR, "Failed to add wifi node: %s", result.status_string());
    return result.status_value();
  }

  return ZX_OK;
}

}  // namespace nelson
