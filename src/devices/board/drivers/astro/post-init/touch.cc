// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.input.focaltech/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pin/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <cinttypes>
#include <string>

#include <bind/fuchsia/amlogic/platform/s905d2/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>

#include "src/devices/board/drivers/astro/post-init/post-init.h"
namespace {

zx::result<> SetPull(const fdf::Namespace& incoming, std::string_view node_name,
                     fuchsia_hardware_pin::Pull pull) {
  zx::result pin = incoming.Connect<fuchsia_hardware_pin::Service::Device>(node_name);
  if (pin.is_error()) {
    fdf::error("Failed to connect to pin node: {}", pin.status_string());
    return pin.take_error();
  }

  fidl::Arena arena;
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).pull(pull).Build();
  fidl::WireResult result = fidl::WireCall(*pin)->Configure(config);
  if (!result.ok()) {
    fdf::error("Call to Configure failed: {}", result.FormatDescription().c_str());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("Configure failed: {}", result.FormatDescription().c_str());
    return result->take_error();
  }
  return zx::ok();
}

}  // namespace

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

const std::vector kFocaltechI2cRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "focaltech"),
};

const std::vector kFocaltechI2cProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
};

const std::vector kGoodixI2cRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "goodix"),
};

const std::vector kGoodixI2cProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
};

const std::vector kInterruptRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                            bind_fuchsia_amlogic_platform_s905d2::GPIOZ_PIN_ID_PIN_4),
};

const std::vector kInterruptProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-int"),
};

const std::vector kResetRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                            bind_fuchsia_amlogic_platform_s905d2::GPIOZ_PIN_ID_PIN_9),
};

const std::vector kResetProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-reset"),
};

const std::vector kGpioInitRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

const std::vector kGpioInitProperties = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

zx::result<> AddFocaltechTouch(
    fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  static const fuchsia_hardware_input_focaltech::Metadata kDeviceInfo({
      .device_id = fuchsia_hardware_input_focaltech::DeviceId::kFt3X27,
      .needs_firmware = false,
  });

  fit::result persisted_metadata = fidl::Persist(kDeviceInfo);
  if (persisted_metadata.is_error()) {
    fdf::error("Failed to persist focaltech metadata: {}",
               persisted_metadata.error_value().FormatDescription().c_str());
    return zx::error(persisted_metadata.error_value().status());
  }

  fpbus::Node node({.name = "focaltech-touch-38",
                    .vid = bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC,
                    .pid = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC,
                    .did = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_FOCALTOUCH,
                    .metadata = std::vector<fpbus::Metadata>{
                        {{
                            .id = fuchsia_hardware_input_focaltech::Metadata::kSerializableName,
                            .data = std::move(persisted_metadata.value()),
                        }},
                    }});

  auto parents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = kFocaltechI2cRules,
          .properties = kFocaltechI2cProperties,
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = kInterruptRules,
          .properties = kInterruptProperties,
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = kResetRules,
          .properties = kResetProperties,
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = kGpioInitRules,
          .properties = kGpioInitProperties,
      }},
  };

  auto composite_node_spec = fuchsia_driver_framework::CompositeNodeSpec{
      {.name = "focaltech-touch-38", .parents2 = parents}};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('FOCL');
  fdf::WireUnownedResult result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, node), fidl::ToWire(fidl_arena, composite_node_spec));
  if (!result.ok()) {
    fdf::error("Failed to send AddCompositeNodeSpec request: {}", result.status_string());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("Failed to add composite node spec: {}",
               zx_status_get_string(result->error_value()));
    return result->take_error();
  }

  return zx::ok();
}

zx::result<> PostInit::InitTouch(const fdf::Namespace& incoming) {
  switch (panel_type_) {
    case display::PanelType::kInnoluxP070acbFitipowerJd9364:
      // Innolux P070ACB panel on Astro uses Goodix touch controller chip.
      return InitGoodixTouch(incoming);
    case display::PanelType::kBoeTv070wsmFitipowerJd9364Astro:
      // BOE TV070WSM panel on Astro uses Focaltech FT3x27 touch controller
      // chip.
      return InitFocaltechTouch(incoming);
    default:
      break;
  }
  fdf::error("Invalid panel type for Astro: {}", static_cast<uint32_t>(panel_type_));
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> PostInit::InitGoodixTouch(const fdf::Namespace& incoming) {
  // The Goodix touch driver expects the interrupt line to be pulled up and the reset line to be
  // pulled down.
  // TODO(https://fxbug.dev/428033669): Move the GPIO initialization
  // logic to the touch driver.
  if (auto result = SetPull(incoming, "touch-interrupt", fuchsia_hardware_pin::Pull::kUp);
      result.is_error()) {
    return result;
  }
  if (auto result = SetPull(incoming, "touch-reset", fuchsia_hardware_pin::Pull::kDown);
      result.is_error()) {
    return result;
  }

  const std::vector<fuchsia_driver_framework::ParentSpec2> goodix_parents{
      {{kGoodixI2cRules, kGoodixI2cProperties}},
      {{kInterruptRules, kInterruptProperties}},
      {{kResetRules, kResetProperties}},
  };

  const fuchsia_driver_framework::CompositeNodeSpec goodix_node_spec{{
      .name = "goodix-touch-5d",
      .parents2 = goodix_parents,
  }};

  if (auto result = composite_manager_->AddSpec(goodix_node_spec); result.is_error()) {
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

zx::result<> PostInit::InitFocaltechTouch(const fdf::Namespace& incoming) {
  // The Focaltech touch driver expects the interrupt line to be driven by the touch controller.
  // TODO(https://fxbug.dev/428033669): Move the GPIO initialization
  // logic to the touch driver.
  if (auto result = SetPull(incoming, "touch-interrupt", fuchsia_hardware_pin::Pull::kNone);
      result.is_error()) {
    return result;
  }

  auto status = AddFocaltechTouch(pbus_);
  if (!status.is_ok()) {
    fdf::error("ft3x27: DdkAddCompositeNodeSpec failed: {}", status.status_string());
    return status;
  }

  return zx::ok();
}

}  // namespace astro
