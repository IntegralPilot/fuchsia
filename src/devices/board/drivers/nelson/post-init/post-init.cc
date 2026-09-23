// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "post-init.h"

#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pin/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/inspect/cpp/reader.h>

#include <array>
#include <cinttypes>

#include <bind/fuchsia/amlogic/platform/s905d3/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/infineon/platform/cpp/bind.h>

#include "sdk/lib/driver/component/cpp/composite_node_spec.h"
#include "sdk/lib/driver/component/cpp/node_add_args.h"

namespace {

constexpr std::array<const char*, 3> kBoardBuildNodeNames = {
    "hw-id-0",
    "hw-id-1",
    "hw-id-2",
};

constexpr std::array<const char*, 2> kBoardOptionNodeNames = {
    "hw-id-3",
    "hw-id-4",
};

constexpr std::array<const char*, 1> kPanelDdicModelNodeNames = {
    // Corresponds to pin `ID0` on Nelson display modules.
    "disp-soc-id0",
};

constexpr std::array<const char*, 1> kPanelVendorNodeNames = {
    // Corresponds to pin `ID1` on Nelson display modules.
    "disp-soc-id1",
};

// Nelson Board Revs
enum {
  kBoardBuildP1 = 0,
  kBoardBuildP2 = 1,
  kBoardBuildP2Doe = 2,
  kBoardBuildPreEvt = 3,
  kBoardBuildEvt = 4,
  kBoardBuildDvt = 5,
  kBoardBuildDvt2 = 6,

  kMaxSupportedBuild,  // This must be last entry
};

}  // namespace

namespace nelson {

namespace {

// Values map to GPIO pin semantics for the display module ID1 pin.
enum class PanelVendor : uint32_t {
  kKd = 0,
  kBoe = 1,
};

// Values map to GPIO pin semantics for the display module ID0 pin.
enum class PanelDdicModel : uint32_t {
  kFitipowerJd9364 = 0,
  kFitipowerJd9365 = 1,
};

zx::result<display::PanelType> GetPanelType(PanelVendor panel_vendor,
                                            PanelDdicModel panel_ddic_model) {
  switch (panel_vendor) {
    case PanelVendor::kKd:
      switch (panel_ddic_model) {
        case PanelDdicModel::kFitipowerJd9364:
          return zx::ok(display::PanelType::kKdKd070d82FitipowerJd9364);
        case PanelDdicModel::kFitipowerJd9365:
          return zx::ok(display::PanelType::kKdKd070d82FitipowerJd9365);
      }
      break;
    case PanelVendor::kBoe:
      switch (panel_ddic_model) {
        case PanelDdicModel::kFitipowerJd9364:
          return zx::ok(display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson);
        case PanelDdicModel::kFitipowerJd9365:
          return zx::ok(display::PanelType::kBoeTv070wsmFitipowerJd9365);
      }
      break;
  }
  fdf::error("Invalid GPIO panel type: panel vendor {} panel DDIC model {}",
             static_cast<uint32_t>(panel_vendor), static_cast<uint32_t>(panel_ddic_model));
  return zx::error(ZX_ERR_INVALID_ARGS);
}

}  // namespace

zx::result<> PostInit::Start(fdf::DriverContext context) {
  auto incoming = context.take_incoming();
  parent_.Bind(take_node());

  zx::result pbus = incoming->Connect<fuchsia_hardware_platform_bus::Service::PlatformBus>("pbus");
  if (pbus.is_error()) {
    fdf::error("Failed to connect to PlatformBus: {}", pbus.status_string());
    return pbus.take_error();
  }
  pbus_.Bind(*std::move(pbus));

  zx::result composite_manager =
      incoming->Connect<fuchsia_driver_framework::CompositeNodeManager>();
  if (composite_manager.is_error()) {
    fdf::error("Failed to connect to CompositeNodeManager: {}", composite_manager.status_string());
    return composite_manager.take_error();
  }
  composite_manager_ = fidl::SyncClient(*std::move(composite_manager));

  auto args = fuchsia_driver_framework::NodeAddArgs({.name = "post-init"});

  zx::result controller_endpoints =
      fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (controller_endpoints.is_error()) {
    fdf::error("Failed to create controller endpoints: {}", controller_endpoints.status_string());
    return controller_endpoints.take_error();
  }
  controller_.Bind(std::move(controller_endpoints->client));

  if (zx::result result = InitBoardInfo(*incoming); result.is_error()) {
    return result.take_error();
  }

  if (zx::result result = SetInspectProperties(*incoming); result.is_error()) {
    return result.take_error();
  }

  if (zx::result result = IdentifyPanel(*incoming); result.is_error()) {
    return result.take_error();
  }

  if (zx::result result = InitDisplay(); result.is_error()) {
    return result.take_error();
  }

  if (zx::result result = InitTouch(*incoming); result.is_error()) {
    return result.take_error();
  }

  if (zx::result result = InitBacklight(); result.is_error()) {
    return result.take_error();
  }

  if (zx::result result = SetBoardInfo(); result.is_error()) {
    return result.take_error();
  }

  if (zx::result result = AddSelinaCompositeNode(*incoming); result.is_error()) {
    return result.take_error();
  }

  auto result = parent_->AddChild({std::move(args), std::move(controller_endpoints->server), {}});
  if (result.is_error()) {
    if (result.error_value().is_framework_error()) {
      fdf::error("Failed to add child: {}",
                 result.error_value().framework_error().FormatDescription().c_str());
      return zx::error(result.error_value().framework_error().status());
    }
    if (result.error_value().is_domain_error()) {
      fdf::error("Failed to add child");
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  return zx::ok();
}

zx::result<> PostInit::InitBoardInfo(const fdf::Namespace& incoming) {
  zx::result<uint32_t> board_build =
      ReadGpios({kBoardBuildNodeNames.data(), kBoardBuildNodeNames.size()}, incoming);
  if (board_build.is_error()) {
    return board_build.take_error();
  }

  fdf::info("Detected board rev 0x{:x}", *board_build);

  if (*board_build >= kMaxSupportedBuild) {
    // We have detected a new board rev. Print this warning just in case the
    // new board rev requires additional support that we were not aware of
    fdf::info("Unsupported board revision detected ({})", *board_build);
  }
  board_build_ = *board_build;

  zx::result<uint32_t> board_option =
      ReadGpios({kBoardOptionNodeNames.data(), kBoardOptionNodeNames.size()}, incoming);
  if (board_option.is_error()) {
    return board_option.take_error();
  }
  board_option_ = *board_option;

  return zx::ok();
}

zx::result<> PostInit::IdentifyPanel(const fdf::Namespace& incoming) {
  zx::result<uint32_t> panel_vendor_result =
      ReadGpios({kPanelVendorNodeNames.data(), kPanelVendorNodeNames.size()}, incoming);
  if (panel_vendor_result.is_error()) {
    fdf::error("Failed to read panel vendor GPIOs: {}", panel_vendor_result.status_string());
    return panel_vendor_result.take_error();
  }
  // The cast result will always be a valid member because the `ReadGpios()`
  // result for a single GPIO is guaranteed to be 0 or 1.
  PanelVendor panel_vendor = static_cast<PanelVendor>(std::move(panel_vendor_result).value());

  zx::result<uint32_t> panel_ddic_model_result =
      ReadGpios({kPanelDdicModelNodeNames.data(), kPanelDdicModelNodeNames.size()}, incoming);
  if (panel_ddic_model_result.is_error()) {
    fdf::error("Failed to read panel DDIC model GPIOs: {}",
               panel_ddic_model_result.status_string());
    return panel_ddic_model_result.take_error();
  }
  PanelDdicModel panel_ddic_model =
      static_cast<PanelDdicModel>(std::move(panel_ddic_model_result).value());

  zx::result<display::PanelType> panel_type_result = GetPanelType(panel_vendor, panel_ddic_model);
  if (panel_type_result.is_error()) {
    fdf::error("Failed to get panel type: {}", panel_type_result.status_string());
    return panel_type_result.take_error();
  }
  panel_type_ = std::move(panel_type_result).value();

  return zx::ok();
}

zx::result<> PostInit::SetInspectProperties(const fdf::Namespace& incoming) {
  auto inspect_sink = incoming.Connect<fuchsia_inspect::InspectSink>();
  if (inspect_sink.is_error() || !inspect_sink->is_valid()) {
    fdf::error("Failed to connect to InspectSink: {}", inspect_sink.status_string());
    return inspect_sink.take_error();
  }

  component_inspector_ = std::make_unique<inspect::ComponentInspector>(
      dispatcher(), inspect::PublishOptions{.inspector = inspector_});

  root_ = inspector_.GetRoot().CreateChild("nelson_board_driver");
  board_build_property_ = root_.CreateUint("board_build", board_build_);
  board_option_property_ = root_.CreateUint("board_option", board_option_);
  panel_type_property_ = root_.CreateUint("panel_type", static_cast<uint32_t>(panel_type_));

  return zx::ok();
}

zx::result<uint32_t> PostInit::ReadGpios(cpp20::span<const char* const> node_names,
                                         const fdf::Namespace& incoming) {
  uint32_t value = 0;

  for (size_t i = 0; i < node_names.size(); i++) {
    zx::result gpio = incoming.Connect<fuchsia_hardware_gpio::Service::Device>(node_names[i]);
    if (gpio.is_error()) {
      fdf::error("Failed to connect to GPIO node: {}", gpio.status_string());
      return gpio.take_error();
    }

    fidl::SyncClient<fuchsia_hardware_gpio::Gpio> gpio_client(*std::move(gpio));

    {
      fidl::Result<fuchsia_hardware_gpio::Gpio::SetBufferMode> result =
          gpio_client->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
      if (result.is_error()) {
        if (result.error_value().is_framework_error()) {
          fdf::error("Call to SetBufferMode failed: {}",
                     result.error_value().framework_error().FormatDescription().c_str());
          return zx::error(result.error_value().framework_error().status());
        }
        if (result.error_value().is_domain_error()) {
          fdf::error("SetBufferMode failed: {}",
                     zx_status_get_string(result.error_value().domain_error()));
          return zx::error(result.error_value().domain_error());
        }

        fdf::error("Unknown error from call to SetBufferMode");
        return zx::error(ZX_ERR_BAD_STATE);
      }
    }

    {
      auto result = gpio_client->Read();
      if (result.is_error()) {
        if (result.error_value().is_framework_error()) {
          fdf::error("Call to Read failed: {}",
                     result.error_value().framework_error().FormatDescription().c_str());
          return zx::error(result.error_value().framework_error().status());
        }
        if (result.error_value().is_domain_error()) {
          fdf::error("Read failed: {}", zx_status_get_string(result.error_value().domain_error()));
          return zx::error(result.error_value().domain_error());
        }

        fdf::error("Unknown error from call to Read");
        return zx::error(ZX_ERR_BAD_STATE);
      }

      if (result->value()) {
        value |= 1 << i;
      }
    }
  }

  return zx::ok(value);
}

zx::result<> PostInit::SetBoardInfo() {
  fdf::Arena arena('PBUS');
  auto board_info = fuchsia_hardware_platform_bus::wire::BoardInfo::Builder(arena)
                        .board_revision(board_build_)
                        .Build();

  auto result = pbus_.buffer(arena)->SetBoardInfo(board_info);
  if (!result.ok()) {
    fdf::error("Call to SetBoardInfo failed: {}", result.FormatDescription().c_str());
    return zx::error(result.error().status());
  }
  if (result->is_error()) {
    fdf::error("SetBoardInfo failed: {}", zx_status_get_string(result->error_value()));
    return result->take_error();
  }

  return zx::ok();
}

zx::result<> PostInit::AddSelinaCompositeNode(const fdf::Namespace& incoming) {
  if (board_build_ == kBoardBuildP1) {
    if (auto result = EnableSelinaOsc(incoming); result.is_error()) {
      return result.take_error();
    }
  }

  const std::vector<fuchsia_driver_framework::BindRule2> spi_rules{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                              bind_fuchsia_infineon_platform::BIND_PLATFORM_DEV_VID_INFINEON),
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                              bind_fuchsia_infineon_platform::BIND_PLATFORM_DEV_PID_BGT60TR13C),
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                              bind_fuchsia_infineon_platform::BIND_PLATFORM_DEV_DID_RADAR_SENSOR),
  };

  const std::vector<fuchsia_driver_framework::NodeProperty2> spi_properties{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                         bind_fuchsia_infineon_platform::BIND_PLATFORM_DEV_VID_INFINEON),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                         bind_fuchsia_infineon_platform::BIND_PLATFORM_DEV_PID_BGT60TR13C),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                         bind_fuchsia_infineon_platform::BIND_PLATFORM_DEV_DID_RADAR_SENSOR),
  };

  const std::vector<fuchsia_driver_framework::BindRule2> irq_gpio_rules{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_s905d3::GPIOH_PIN_ID_PIN_3),
  };

  const std::vector<fuchsia_driver_framework::NodeProperty2> irq_gpio_properties{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "irq-gpio"),
  };

  const std::vector<fuchsia_driver_framework::BindRule2> reset_gpio_rules{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_s905d3::GPIOH_PIN_ID_PIN_2),
  };

  const std::vector<fuchsia_driver_framework::NodeProperty2> reset_gpio_properties{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "reset-gpio"),
  };

  const std::vector<fuchsia_driver_framework::BindRule2> cs_gpio_rules{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_s905d3::GPIOH_PIN_ID_PIN_6),
  };

  const std::vector<fuchsia_driver_framework::NodeProperty2> cs_gpio_properties{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "spi-cs-gpio"),
  };

  const std::vector<fuchsia_driver_framework::ParentSpec2> selina_parents{
      {{spi_rules, spi_properties}},
      {{irq_gpio_rules, irq_gpio_properties}},
      {{reset_gpio_rules, reset_gpio_properties}},
      {{cs_gpio_rules, cs_gpio_properties}},
  };

  const fuchsia_driver_framework::CompositeNodeSpec selina_node_spec{{
      .name = "selina-composite",
      .parents2 = selina_parents,
  }};

  if (auto result = composite_manager_->AddSpec(selina_node_spec); result.is_error()) {
    if (result.error_value().is_framework_error()) {
      fdf::error("Call to AddSpec failed: {}",
                 result.error_value().framework_error().FormatDescription().c_str());
      return zx::error(result.error_value().framework_error().status());
    }
    if (result.error_value().is_domain_error()) {
      fdf::error("AddSpec failed");
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  return zx::ok();
}

zx::result<> PostInit::EnableSelinaOsc(const fdf::Namespace& incoming) {
  zx::result gpio = incoming.Connect<fuchsia_hardware_gpio::Service::Device>("selina-osc-en");
  if (gpio.is_error()) {
    fdf::error("Failed to connect to GPIO node: {}", gpio.status_string());
    return gpio.take_error();
  }

  fidl::SyncClient osc_en_gpio(*std::move(gpio));

  zx::result pin = incoming.Connect<fuchsia_hardware_pin::Service::Device>("selina-osc-en");
  if (pin.is_error()) {
    fdf::error("Failed to connect to GPIO node: {}", pin.status_string());
    return pin.take_error();
  }

  fidl::SyncClient osc_en_pin(*std::move(pin));

  fuchsia_hardware_pin::Configuration config{{
      .pull = fuchsia_hardware_pin::Pull::kNone,
      .function = 0,
  }};
  if (auto result = osc_en_pin->Configure(std::move(config)); result.is_error()) {
    if (result.error_value().is_framework_error()) {
      fdf::error("Call to set SELINA_OSC_EN configuraiton failed: {}",
                 result.error_value().framework_error().FormatDescription().c_str());
      return zx::error(result.error_value().framework_error().status());
    }
    if (result.error_value().is_domain_error()) {
      fdf::error("Failed to set SELINA_OSC_EN configuration: {}",
                 zx_status_get_string(result.error_value().domain_error()));
      return zx::error(result.error_value().domain_error());
    }
  }

  if (auto result = osc_en_gpio->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
      result.is_error()) {
    if (result.error_value().is_framework_error()) {
      fdf::error("Call to set SELINA_OSC_EN to input failed: {}",
                 result.error_value().framework_error().FormatDescription().c_str());
      return zx::error(result.error_value().framework_error().status());
    }
    if (result.error_value().is_domain_error()) {
      fdf::error("Failed to set SELINA_OSC_EN to input function: {}",
                 zx_status_get_string(result.error_value().domain_error()));
      return zx::error(result.error_value().domain_error());
    }
  }

  return zx::ok();
}

}  // namespace nelson

FUCHSIA_DRIVER_EXPORT2(nelson::PostInit);
