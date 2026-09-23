// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/board/drivers/astro/post-init/post-init.h"

#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

#include <cinttypes>

namespace astro {

namespace {

constexpr std::array<const char*, 3> kBoardBuildNodeNames = {
    "hw-id-0",
    "hw-id-1",
    "hw-id-2",
};

constexpr std::array<const char*, 1> kDisplayPanelVendorNodeNames = {
    "disp-soc-vid",
};

enum class PanelVendor : uint8_t {
  kBoe = 0,
  kInnolux = 1,
};

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

  if (zx::result result = SetBoardInfo(); result.is_error()) {
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
  if (zx::result<uint8_t> board_build = ReadGpios(kBoardBuildNodeNames, incoming);
      board_build.is_ok()) {
    board_build_ = static_cast<AstroBoardBuild>(*board_build);
  } else {
    return board_build.take_error();
  }

  if (board_build_ != BOARD_REV_DVT && board_build_ != BOARD_REV_PVT) {
    fdf::error("Unsupported board revision {}", static_cast<uint32_t>(board_build_));
  } else {
    fdf::info("Detected board rev 0x{:x}", static_cast<uint32_t>(board_build_));
  }
  return zx::ok();
}

zx::result<> PostInit::IdentifyPanel(const fdf::Namespace& incoming) {
  zx::result<uint8_t> panel_vendor_result = ReadGpios(kDisplayPanelVendorNodeNames, incoming);
  if (panel_vendor_result.is_error()) {
    fdf::error("Failed to read display panel vendor: {}", panel_vendor_result.status_string());
    return panel_vendor_result.take_error();
  }

  PanelVendor panel_vendor = static_cast<PanelVendor>(std::move(panel_vendor_result).value());
  switch (panel_vendor) {
    case PanelVendor::kBoe:
      panel_type_ = display::PanelType::kBoeTv070wsmFitipowerJd9364Astro;
      return zx::ok();
    case PanelVendor::kInnolux:
      panel_type_ = display::PanelType::kInnoluxP070acbFitipowerJd9364;
      return zx::ok();
  }
  fdf::error("Invalid panel vendor: {}", static_cast<uint8_t>(panel_vendor));
  return zx::error(ZX_ERR_NOT_SUPPORTED);
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

zx::result<uint8_t> PostInit::ReadGpios(cpp20::span<const char* const> node_names,
                                        const fdf::Namespace& incoming) {
  uint8_t value = 0;

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
      fidl::Result<fuchsia_hardware_gpio::Gpio::Read> result = gpio_client->Read();
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

}  // namespace astro

FUCHSIA_DRIVER_EXPORT2(astro::PostInit);
