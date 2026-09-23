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
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zx/handle.h>

#include <optional>

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
#include <soc/aml-t931/t931-gpio.h>
#include <soc/aml-t931/t931-hw.h>

#include "sherlock.h"
#include "src/devices/lib/broadcom/commands.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

namespace {

uint32_t GpioBase() {
  return fbl::round_down<uint32_t, uint32_t>(T931_GPIO_BASE, zx_system_get_page_size());
}
uint32_t GpioBaseOffset() { return T931_GPIO_BASE - GpioBase(); }

class PadDsReg2A : public hwreg::RegisterBase<PadDsReg2A, uint32_t> {
 public:
  static constexpr uint32_t kDriveStrengthMax = 3;

  static auto Get() { return hwreg::RegisterAddr<PadDsReg2A>((0xd2 * 4) + GpioBaseOffset()); }

  DEF_FIELD(1, 0, gpiox_0_select);
  DEF_FIELD(3, 2, gpiox_1_select);
  DEF_FIELD(5, 4, gpiox_2_select);
  DEF_FIELD(7, 6, gpiox_3_select);
  DEF_FIELD(9, 8, gpiox_4_select);
  DEF_FIELD(11, 10, gpiox_5_select);
};

static const std::vector<fpbus::BootMetadata> wifi_boot_metadata{
    {{
        .zbi_type = ZBI_TYPE_DRV_MAC_ADDRESS,
        .zbi_extra = MACADDR_WIFI,
    }},
};

static const std::vector<fpbus::Mmio> sd_emmc_mmios{
    {{
        .base = T931_SD_EMMC_A_BASE,
        .length = T931_SD_EMMC_A_LENGTH,
    }},
};

static const std::vector<fpbus::Irq> sd_emmc_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_SD_EMMC_A_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

static const std::vector<fpbus::Bti> sd_emmc_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_SDIO,
    }},
};

const std::vector<fuchsia_driver_framework::BindRule2> kPwmRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM),
};

const std::vector<fuchsia_driver_framework::NodeProperty2> kPwmProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM),
};

const std::vector<fuchsia_driver_framework::BindRule2> kGpioResetRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(T931_WIFI_REG_ON)),
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
              {"WW", 1},   {"AU", 923}, {"CA", 901}, {"US", 843}, {"GB", 889}, {"BE", 889},
              {"BG", 889}, {"CZ", 889}, {"DK", 889}, {"DE", 889}, {"EE", 889}, {"IE", 889},
              {"GR", 889}, {"ES", 889}, {"FR", 889}, {"HR", 889}, {"IT", 889}, {"CY", 889},
              {"LV", 889}, {"LT", 889}, {"LU", 889}, {"HU", 889}, {"MT", 889}, {"NL", 889},
              {"AT", 889}, {"PL", 889}, {"PT", 889}, {"RO", 889}, {"SI", 889}, {"SK", 889},
              {"FI", 889}, {"SE", 889}, {"EL", 889}, {"IS", 889}, {"LI", 889}, {"TR", 889},
              {"CH", 889}, {"NO", 889}, {"JP", 2},   {"", 0},
          },
  }};

  static const std::vector<fuchsia_driver_framework::BindRule2> kGpioWifiHostRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(T931_WIFI_HOST_WAKE)),
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
      .name = "wifi-1",
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
                                   {.name = "wifi-1", .parents2 = wifi_parents}}));
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

  fpbus::Node node{{
      .name = "mmc-ffe03000",
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
              fidl::ToWire(fidl_arena, node),
              fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                           {.name = "mmc-ffe03000", .parents2 = kSdioParents}}));

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

}  // namespace

zx_status_t Sherlock::SdioInit() {
  // Configure eMMC-SD soc pads.
  gpio_init_steps_.push_back(GpioFunction(T931_SDIO_D0, T931_SDIO_D0_FN));
  gpio_init_steps_.push_back(GpioFunction(T931_SDIO_D1, T931_SDIO_D1_FN));
  gpio_init_steps_.push_back(GpioFunction(T931_SDIO_D2, T931_SDIO_D2_FN));
  gpio_init_steps_.push_back(GpioFunction(T931_SDIO_D3, T931_SDIO_D3_FN));
  gpio_init_steps_.push_back(GpioFunction(T931_SDIO_CLK, T931_SDIO_CLK_FN));
  gpio_init_steps_.push_back(GpioFunction(T931_SDIO_CMD, T931_SDIO_CMD_FN));

  const size_t rounded_size =
      fbl::round_up<size_t>(GpioBaseOffset() + T931_GPIO_LENGTH, zx_system_get_page_size());
  zx::unowned_resource res(get_mmio_resource(parent()));
  zx::vmo vmo;
  zx_status_t status = zx::vmo::create_physical(*res, GpioBase(), rounded_size, &vmo);
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to create VMO: %s", zx_status_get_string(status));
    return status;
  }
  zx::result<fdf::MmioBuffer> buf = fdf::MmioBuffer::Create(
      0, GpioBaseOffset() + T931_GPIO_LENGTH, std::move(vmo), ZX_CACHE_POLICY_UNCACHED_DEVICE);

  if (buf.is_error()) {
    zxlogf(ERROR, "fdf::MmioBuffer::Create() error: %s", buf.status_string());
    return buf.status_value();
  }

  PadDsReg2A::Get()
      .ReadFrom(&(*buf))
      .set_gpiox_0_select(PadDsReg2A::kDriveStrengthMax)
      .set_gpiox_1_select(PadDsReg2A::kDriveStrengthMax)
      .set_gpiox_2_select(PadDsReg2A::kDriveStrengthMax)
      .set_gpiox_3_select(PadDsReg2A::kDriveStrengthMax)
      .set_gpiox_4_select(PadDsReg2A::kDriveStrengthMax)
      .set_gpiox_5_select(PadDsReg2A::kDriveStrengthMax)
      .WriteTo(&(*buf));

  gpio_init_steps_.push_back(GpioFunction(T931_WIFI_REG_ON, T931_WIFI_REG_ON_FN));
  gpio_init_steps_.push_back(GpioFunction(T931_WIFI_HOST_WAKE, T931_WIFI_HOST_WAKE_FN));

  gpio_init_steps_.push_back(GpioPull(T931_WIFI_HOST_WAKE, fuchsia_hardware_pin::Pull::kNone));

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

}  // namespace sherlock
