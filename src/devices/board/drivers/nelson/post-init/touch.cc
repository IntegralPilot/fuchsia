// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pin/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/zx/result.h>

#include <bind/fuchsia/amlogic/platform/s905d3/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>

#include "post-init.h"

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;

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

const std::vector kI2cRules = {
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "goodix"),
};

const std::vector kI2cProperties = {
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
};

const std::vector kInterruptRules = {
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                            bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_4),
};

const std::vector kInterruptProperties = {
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-int"),
};

const std::vector kResetRules = {
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                            bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_9),
};

const std::vector kResetProperties = {
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-reset"),
};

const std::vector kGpioInitRules = {
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

const std::vector kGpioInitProperties = {
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};
}  // namespace

zx::result<> PostInit::InitTouch(const fdf::Namespace& incoming) {
  // The Goodix touch driver expects the interrupt line to be driven by the touch controller.
  if (auto result = SetPull(incoming, "touch-interrupt", fuchsia_hardware_pin::Pull::kNone);
      result.is_error()) {
    return result;
  }

  const std::vector<fpbus::Metadata> display_panel_metadata{
      {{
          .id = std::to_string(DEVICE_METADATA_DISPLAY_PANEL_TYPE),
          .data = std::vector<uint8_t>(
              reinterpret_cast<const uint8_t*>(&panel_type_),
              reinterpret_cast<const uint8_t*>(&panel_type_) + sizeof(display::PanelType)),
      }},
  };

  fpbus::Node touch_dev;
  touch_dev.name() = "goodix-gt6853-touch";
  touch_dev.vid() = PDEV_VID_GOODIX;
  touch_dev.did() = PDEV_DID_GOODIX_GT6853;
  touch_dev.metadata() = display_panel_metadata;

  auto parents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = kI2cRules,
          .properties = kI2cProperties,
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
      {.name = "goodix_gt6853_touch", .parents2 = parents}};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('TOUC');
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, touch_dev), fidl::ToWire(fidl_arena, composite_node_spec));
  if (!result.ok()) {
    fdf::error("AddCompositeNodeSpec Touch(touch_dev) request failed: {}",
               result.FormatDescription().data());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("AddCompositeNodeSpec Touch(touch_dev) failed: {}",
               zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }
  return zx::ok();
}

}  // namespace nelson
