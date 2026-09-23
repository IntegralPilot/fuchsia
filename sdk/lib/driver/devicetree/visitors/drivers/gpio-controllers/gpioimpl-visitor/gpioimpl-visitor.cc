// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/driver/devicetree/visitors/drivers/gpio-controllers/gpioimpl-visitor/gpioimpl-visitor.h"

#include <ctype.h>
#include <fidl/fuchsia.hardware.pinimpl/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <algorithm>
#include <cstdint>
#include <format>
#include <memory>
#include <optional>
#include <set>
#include <string_view>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>

namespace gpio_impl_dt {

namespace {

using fuchsia_hardware_gpio::BufferMode;
using fuchsia_hardware_pin::DriveType;
using fuchsia_hardware_pin::Pull;
using fuchsia_hardware_pinimpl::InitCall;
using fuchsia_hardware_pinimpl::Metadata;

class GpioCells {
 public:
  explicit GpioCells(fdf_devicetree::PropertyCells cells) : gpio_cells_(cells, 1, 1) {}

  // 1st cell denotes the gpio pin.
  uint32_t pin() { return static_cast<uint32_t>(*gpio_cells_[0][0]); }

  // 2nd cell represents GpioFlags. This is only used in gpio init hog nodes and ignored elsewhere.
  zx::result<Pull> flags() {
    switch (static_cast<uint32_t>(*gpio_cells_[0][1])) {
      case 0:
        return zx::ok(Pull::kDown);
      case 1:
        return zx::ok(Pull::kUp);
      case 2:
        return zx::ok(Pull::kNone);
      default:
        return zx::error(ZX_ERR_INVALID_ARGS);
    };
  }

 private:
  using GpioElement = devicetree::PropEncodedArrayElement<2>;
  devicetree::PropEncodedArray<GpioElement> gpio_cells_;
};

}  // namespace

// TODO(b/325077980): Name of the reference property can be *-gpios.
GpioImplVisitor::GpioImplVisitor() {
  fdf_devicetree::Properties gpio_properties = {};
  gpio_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kGpioReference, kGpioCells, /* required */ false));
  gpio_properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kGpioNames, /* required */ false));
  gpio_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(gpio_properties));
}

zx::result<> GpioImplVisitor::Visit(fdf_devicetree::Node& node,
                                    const devicetree::PropertyDecoder& decoder) {
  if (node.GetProperty<bool>("gpio-hog")) {
    // Node containing gpio-hog property are to be parsed differently. They will be used to
    // construct gpio init step metadata.
    auto result = ParseGpioHogChild(node);
    if (result.is_error()) {
      fdf::error("Gpio visitor failed for node '{}' : {}", node.name(), result);
    }
  } else {
    auto gpio_props = gpio_parser_->Parse(node);
    if (gpio_props.is_error()) {
      return gpio_props.take_error();
    }

    auto gpios = gpio_props->Get<fdf_devicetree::References>(kGpioReference);
    if (gpios) {
      auto gpio_names = gpio_props->Get<std::vector<std::string>>(kGpioNames);
      if (!gpio_names || gpio_names->size() != gpios->size()) {
        // We need a gpio names to generate bind rules.
        fdf::error("Gpio reference '{}' does not have valid gpio names field.", node.name());

        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      for (uint32_t index = 0; index < gpios->size(); index++) {
        auto& reference = (*gpios)[index];
        if (is_match(reference.reference_node().properties())) {
          auto result = ParseReferenceChild(node, reference.reference_node(),
                                            reference.property_cells(), (*gpio_names)[index]);
          if (result.is_error()) {
            return result.take_error();
          }
        }
      }
    }

    bool has_names = node.GetProperty<std::vector<std::string>>("pinctrl-names").is_ok();
    if (has_names) {
      auto result = ParsePinStates(node);
      if (result.is_error()) {
        return result.take_error();
      }
    } else {
      auto result = ParseBootTimeConfig(node);
      if (result.is_error()) {
        return result.take_error();
      }
    }
  }

  return zx::ok();
}

zx::result<> GpioImplVisitor::ParsePinStates(fdf_devicetree::Node& node) {
  auto names_prop = node.GetProperty<std::vector<std::string>>("pinctrl-names");
  if (names_prop.is_error()) {
    return names_prop.take_error();
  }
  std::vector<std::string> state_names = *names_prop;

  uint32_t num_states = 0;
  while (true) {
    std::string prop_name = "pinctrl-" + std::to_string(num_states);
    if (node.properties().contains(prop_name)) {
      num_states++;
    } else {
      break;
    }
  }

  if (num_states == 0) {
    return zx::ok();
  }

  if (state_names.size() != num_states) {
    fdf::error("Node '{}' has {} pin states but {} names in pinctrl-names.", node.name(),
               num_states, state_names.size());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fdf_devicetree::Properties pinctrl_properties;
  for (uint32_t i = 0; i < num_states; ++i) {
    pinctrl_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
        "pinctrl-" + std::to_string(i), 0u, false));
  }
  fdf_devicetree::PropertyParser pinctrl_parser(std::move(pinctrl_properties));
  auto pinctrl_props = pinctrl_parser.Parse(node);
  if (pinctrl_props.is_error()) {
    return pinctrl_props.take_error();
  }

  // Collect all unique GPIO controller IDs referenced by the pin states.
  std::set<uint32_t> controllers;
  for (uint32_t i = 0; i < num_states; ++i) {
    std::string prop_name = "pinctrl-" + std::to_string(i);
    auto pinctrl_configs = pinctrl_props->Get<fdf_devicetree::References>(prop_name);
    if (!pinctrl_configs) {
      fdf::error("Failed to get pinctrl reference property '{}' for node '{}'", prop_name,
                 node.name());
      return zx::error(ZX_ERR_INTERNAL);
    }
    for (auto& pinctrl_cfg : *pinctrl_configs) {
      auto gpio_node = GetGpioNodeForPinConfig(pinctrl_cfg.reference_node());
      if (gpio_node.is_error()) {
        return gpio_node.take_error();
      }
      if (!is_match(gpio_node->properties())) {
        continue;
      }
      controllers.insert(gpio_node->id());
    }
  }

  std::map<uint32_t, fuchsia_hardware_pinimpl::DevicePinStates> controller_to_device_states;
  for (uint32_t controller_id : controllers) {
    auto& dev_states = controller_to_device_states[controller_id];
    dev_states.name() = GetUniqueNodeName(node);
    for (uint32_t i = 0; i < num_states; ++i) {
      fuchsia_hardware_pinimpl::PinState state{{
          .name = state_names[i],
          .pins = {},
      }};
      dev_states.states().push_back(std::move(state));
    }
  }

  // Parse configurations for each pin state and add them to the metadata.
  for (uint32_t i = 0; i < num_states; ++i) {
    std::string prop_name = "pinctrl-" + std::to_string(i);
    auto pinctrl_configs = pinctrl_props->Get<fdf_devicetree::References>(prop_name);
    if (!pinctrl_configs) {
      fdf::error("Failed to get pinctrl reference property '{}' for node '{}'", prop_name,
                 node.name());
      return zx::error(ZX_ERR_INTERNAL);
    }

    for (auto& pinctrl_cfg : *pinctrl_configs) {
      auto gpio_node = GetGpioNodeForPinConfig(pinctrl_cfg.reference_node());
      if (gpio_node.is_error()) {
        return gpio_node.take_error();
      }

      if (!is_match(gpio_node->properties())) {
        continue;
      }

      auto& dev_states = controller_to_device_states[gpio_node->id()];
      auto& state = dev_states.states()[i];

      auto result = ParsePinCtrlStateCfg(pinctrl_cfg.reference_node(), *gpio_node, state.pins());
      if (result.is_error()) {
        return result.take_error();
      }
    }
  }

  uint32_t controller_index = 0;
  for (auto& [controller_id, dev_states] : controller_to_device_states) {
    std::string unique_name = dev_states.name();
    auto& controller = GetController(controller_id);
    if (!controller.metadata.device_pin_states()) {
      controller.metadata.device_pin_states().emplace();
    }
    controller.metadata.device_pin_states()->push_back(std::move(dev_states));

    auto result = AddPinStatesNodeSpec(node, controller_id, controller_index++, unique_name);
    if (result.is_error()) {
      return result.take_error();
    }
  }

  return zx::ok();
}

zx::result<> GpioImplVisitor::ParseBootTimeConfig(fdf_devicetree::Node& node) {
  fdf_devicetree::Properties pinctrl_properties;
  pinctrl_properties.emplace_back(
      std::make_unique<fdf_devicetree::ReferenceProperty>(kPinCtrl0, 0u, false));
  fdf_devicetree::PropertyParser pinctrl_parser(std::move(pinctrl_properties));
  auto pinctrl_props = pinctrl_parser.Parse(node);
  if (pinctrl_props.is_error()) {
    return pinctrl_props.take_error();
  }

  auto pinctrl_configs = pinctrl_props->Get<fdf_devicetree::References>(kPinCtrl0);
  if (!pinctrl_configs) {
    return zx::ok();
  }

  std::vector<uint32_t> controllers;
  uint32_t controller_index = 0;
  for (auto& pinctrl_cfg : *pinctrl_configs) {
    auto gpio_node = GetGpioNodeForPinConfig(pinctrl_cfg.reference_node());
    if (gpio_node.is_error()) {
      return gpio_node.take_error();
    }
    auto result = ParsePinCtrlCfg(node, pinctrl_cfg.reference_node(), *gpio_node);
    if (result.is_error()) {
      return result.take_error();
    }
    if (std::find(controllers.begin(), controllers.end(), gpio_node->id()) == controllers.end()) {
      result = AddInitNodeSpec(node, gpio_node->id(), controller_index++);
      if (result.is_error()) {
        return result.take_error();
      }
      controllers.push_back(gpio_node->id());
    }
  }

  return zx::ok();
}

zx::result<> GpioImplVisitor::AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t global_id,
                                               const std::string& gpio_name) {
  auto gpio_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, global_id),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, gpio_name),
          },
  }};
  child.AddNodeSpec(gpio_node);
  return zx::ok();
}

zx::result<> GpioImplVisitor::AddInitNodeSpec(fdf_devicetree::Node& child, uint32_t controller_id,
                                              uint32_t controller_index) {
  auto gpio_init_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP,
                                      bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, controller_id),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
              fdf::MakeProperty2(bind_fuchsia::ID, controller_index),
              fdf::MakeProperty2(bind_fuchsia::NAME, std::format("gpio-init-{}", controller_index)),
          },
  }};
  child.AddNodeSpec(gpio_init_node);
  return zx::ok();
}

zx::result<> GpioImplVisitor::AddPinStatesNodeSpec(fdf_devicetree::Node& child,
                                                   uint32_t controller_id,
                                                   uint32_t controller_index,
                                                   const std::string& client_name) {
  auto pin_states_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE,
                                      "fuchsia.hardware.pin.PinStatesService"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, controller_id),
              fdf::MakeAcceptBindRule(bind_fuchsia::NAME, client_name),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pin.PinStatesService"),
              fdf::MakeProperty2(bind_fuchsia::ID, controller_index),
              fdf::MakeProperty2(bind_fuchsia::NAME,
                                 std::format("pin-states-{}", controller_index)),
          },
  }};
  child.AddNodeSpec(pin_states_node);
  return zx::ok();
}

std::string GpioImplVisitor::GetUniqueNodeName(fdf_devicetree::Node& node) {
  std::string name = node.fdf_name();
  auto parent = node.parent();
  if (parent && parent.name() != "dt-root") {
    name = parent.fdf_name() + "-" + name;
  }
  return name;
}

zx::result<fuchsia_hardware_pin::Configuration> GpioImplVisitor::ParsePinConfiguration(
    fdf_devicetree::ReferenceNode& cfg_node) {
  fuchsia_hardware_pin::Configuration config;

  std::optional<Pull> pull;
  auto save_pull = [&](Pull val) -> zx::result<> {
    if (pull.has_value()) {
      fdf::error(
          "Pin controller config '{}' can only support one pull direction. Previously already set with {}, now trying to set as {}",
          cfg_node.name(), static_cast<uint32_t>(*pull), static_cast<uint32_t>(val));
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    pull = val;
    return zx::ok();
  };
  if (cfg_node.GetProperty<bool>(kPinBiasPullDown)) {
    auto result = save_pull(Pull::kDown);
    if (result.is_error())
      return result.take_error();
  }
  if (cfg_node.GetProperty<bool>(kPinBiasPullUp)) {
    auto result = save_pull(Pull::kUp);
    if (result.is_error())
      return result.take_error();
  }
  if (cfg_node.GetProperty<bool>(kPinBiasDisable)) {
    auto result = save_pull(Pull::kNone);
    if (result.is_error())
      return result.take_error();
  }

  config.pull(pull);

  if (auto function = cfg_node.GetProperty<uint64_t>(kPinFunctionId); function.is_ok()) {
    config.function(function.value());
  } else if (function.is_error() && function.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::error("Pin controller config '{}' has invalid function: {}.", cfg_node.name(), function);
    return function.take_error();
  }

  if (auto function_name = cfg_node.GetProperty<std::string>(kPinFunction); function_name.is_ok()) {
    if (config.function().has_value()) {
      fdf::error("Pin controller config '{}' specifies function and function name.",
                 cfg_node.name());
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    config.function_name(function_name.value());
  } else if (function_name.is_error() && function_name.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::error("Pin controller config '{}' has invalid function name: {}.", cfg_node.name(),
               function_name);
    return function_name.take_error();
  }

  auto drive_strength_ua = cfg_node.GetProperty<uint64_t>(kPinDriveStrengthUa);
  if (drive_strength_ua.is_ok()) {
    config.drive_strength_ua(drive_strength_ua.value());
  } else if (drive_strength_ua.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::error("Pin controller config '{}' has invalid drive strength: {}.", cfg_node.name(),
               drive_strength_ua);
    return drive_strength_ua.take_error();
  }

  auto drive_strength = cfg_node.GetProperty<uint32_t>(kPinDriveStrength);
  if (drive_strength.is_ok()) {
    if (config.drive_strength_ua().has_value()) {
      fdf::error("Pin controller config '{}' specifies drive-strength and drive-strength-microamp.",
                 cfg_node.name());
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    config.drive_strength(drive_strength.value());
  } else if (drive_strength.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::error("Pin controller config '{}' has invalid drive strength: {}.", cfg_node.name(),
               drive_strength);
    return drive_strength.take_error();
  }

  std::optional<DriveType> drive_type;
  auto save_drive_type = [&](DriveType val) -> zx::result<> {
    if (drive_type.has_value()) {
      fdf::error(
          "Pin controller config '{}' can only support one drive type. Previously already set with {}, now trying to set as {}",
          cfg_node.name(), static_cast<uint32_t>(*drive_type), static_cast<uint32_t>(val));
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    drive_type = val;
    return zx::ok();
  };
  if (cfg_node.GetProperty<bool>(kPinDrivePushPull)) {
    auto result = save_drive_type(DriveType::kPushPull);
    if (result.is_error())
      return result.take_error();
  }
  if (cfg_node.GetProperty<bool>(kPinDriveOpenDrain)) {
    auto result = save_drive_type(DriveType::kOpenDrain);
    if (result.is_error())
      return result.take_error();
  }
  if (cfg_node.GetProperty<bool>(kPinDriveOpenSource)) {
    auto result = save_drive_type(DriveType::kOpenSource);
    if (result.is_error())
      return result.take_error();
  }
  if (drive_type.has_value()) {
    config.drive_type(drive_type);
  }

  auto power_source = cfg_node.GetProperty<uint32_t>(kPinPowerSource);
  if (power_source.is_ok()) {
    config.power_source(*power_source);
  } else if (power_source.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::error("Pin controller config '{}' has invalid power source: {}.", cfg_node.name(),
               power_source);
    return power_source.take_error();
  }

  if (cfg_node.GetProperty<bool>(kPinWakeVector)) {
    config.wake_vector(true);
  }

  return zx::ok(config);
}

zx::result<std::optional<fuchsia_hardware_gpio::BufferMode>> GpioImplVisitor::ParseBufferMode(
    fdf_devicetree::ReferenceNode& cfg_node) {
  std::optional<fuchsia_hardware_gpio::BufferMode> buffer_mode;
  if (cfg_node.GetProperty<bool>(kPinOutputDisable)) {
    buffer_mode = fuchsia_hardware_gpio::BufferMode::kInput;
  }
  if (cfg_node.GetProperty<bool>(kPinOutputLow)) {
    if (buffer_mode) {
      fdf::error(
          "Multiple values for BufferMode defined in pin config '{}'. Property 'output-low' clashes.",
          cfg_node.name());
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
    buffer_mode = fuchsia_hardware_gpio::BufferMode::kOutputLow;
  }
  if (cfg_node.GetProperty<bool>(kPinOutputHigh)) {
    if (buffer_mode) {
      fdf::error(
          "Multiple values for BufferMode defined in pin config '{}'. Property 'output-high' clashes.",
          cfg_node.name());
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
    buffer_mode = fuchsia_hardware_gpio::BufferMode::kOutputHigh;
  }
  return zx::ok(buffer_mode);
}

zx::result<> GpioImplVisitor::ParsePinCtrlStateCfg(
    fdf_devicetree::ReferenceNode& cfg_node, fdf_devicetree::ParentNode& gpio_node,
    std::vector<fuchsia_hardware_pinimpl::PinConfiguration>& pin_configs) {
  if (!is_match(gpio_node.properties())) {
    return zx::ok();
  }

  auto pins = cfg_node.GetProperty<std::vector<uint32_t>>(kPins);
  if (pins.is_error()) {
    fdf::error("Pin controller config '{}' does not have pins property: {}", cfg_node.name(), pins);
    return pins.take_error();
  }

  if (pins->empty()) {
    fdf::error("No pins found in pin controller config '{}'", cfg_node.name());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto config = ParsePinConfiguration(cfg_node);
  if (config.is_error()) {
    return config.take_error();
  }

  auto buffer_mode = ParseBufferMode(cfg_node);
  if (buffer_mode.is_error()) {
    return buffer_mode.take_error();
  }

  auto actual_buffer_mode = *buffer_mode;
  for (size_t i = 0; i < pins->size(); i++) {
    if (!config->IsEmpty()) {
      fuchsia_hardware_pinimpl::PinConfiguration pin_cfg{{
          .pin = (*pins)[i],
          .call = fuchsia_hardware_pinimpl::InitCall::WithPinConfig(*config),
      }};
      pin_configs.push_back(std::move(pin_cfg));
    }
    if (actual_buffer_mode.has_value()) {
      fuchsia_hardware_pinimpl::PinConfiguration pin_cfg{{
          .pin = (*pins)[i],
          .call = fuchsia_hardware_pinimpl::InitCall::WithBufferMode(*actual_buffer_mode),
      }};
      pin_configs.push_back(std::move(pin_cfg));
    }
  }

  return zx::ok();
}

zx::result<fdf_devicetree::ParentNode> GpioImplVisitor::GetGpioNodeForPinConfig(
    fdf_devicetree::ReferenceNode& cfg_node) {
  // TODO(b/325077980): Add gpio-ranges based mapping in case the pinctrl cfg is not a direct
  // child of gpio-controller.
  return zx::ok(cfg_node.parent());
}

zx::result<> GpioImplVisitor::ParsePinCtrlCfg(fdf_devicetree::Node& child,
                                              fdf_devicetree::ReferenceNode& cfg_node,
                                              fdf_devicetree::ParentNode& gpio_node) {
  // Check that the parent is indeed a gpio-controller that we support.
  if (!is_match(gpio_node.properties())) {
    return zx::ok();
  }

  auto& controller = GetController(gpio_node.id());
  auto pins = cfg_node.GetProperty<std::vector<uint32_t>>(kPins);
  if (pins.is_error()) {
    fdf::error("Pin controller config '{}' does not have pins property: {}", cfg_node.name(), pins);
    return pins.take_error();
  }

  if (pins->empty()) {
    fdf::error("No pins found in pin controller config '{}'", cfg_node.name());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto config = ParsePinConfiguration(cfg_node);
  if (config.is_error()) {
    return config.take_error();
  }

  auto buffer_mode = ParseBufferMode(cfg_node);
  if (buffer_mode.is_error()) {
    return buffer_mode.take_error();
  }

  std::vector<InitCall> init_calls;
  if (!config->IsEmpty()) {
    init_calls.emplace_back(InitCall::WithPinConfig(std::move(*config)));
  }
  if (*buffer_mode) {
    init_calls.emplace_back(InitCall::WithBufferMode(**buffer_mode));
  }
  if (init_calls.empty()) {
    fdf::error("Pin controller config '{}' does not have a valid config.", cfg_node.name());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // Add the init steps for all the pins in the config.
  for (size_t i = 0; i < pins->size(); i++) {
    fdf::debug("Gpio init steps (count: {}) for child '{}' (pin {:#x}) added to controller '{}'",
               init_calls.size(), child.name(), (*pins)[i], gpio_node.name());

    for (auto& init_call : init_calls) {
      auto step = fuchsia_hardware_pinimpl::InitStep::WithCall({{(*pins)[i], init_call}});
      controller.metadata.init_steps()->emplace_back(step);
    }
  }

  return zx::ok();
}

zx::result<> GpioImplVisitor::ParseGpioHogChild(fdf_devicetree::Node& child) {
  auto parent = child.parent().MakeReferenceNode();
  // Check that the parent is indeed a gpio-controller that we support.
  if (!is_match(parent.properties())) {
    return zx::ok();
  }

  auto& controller = GetController(parent.id());
  auto gpios = child.properties().find("gpios");
  if (gpios == child.properties().end()) {
    fdf::error("Gpio init hog '{}' does not have gpios property", child.name());

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  std::optional<fuchsia_hardware_gpio::BufferMode> buffer_mode;

  if (child.GetProperty<bool>("input")) {
    buffer_mode = fuchsia_hardware_gpio::BufferMode::kInput;
  }
  if (child.GetProperty<bool>("output-low")) {
    if (buffer_mode) {
      fdf::error("Gpio init hog '{}' has more than one buffer mode property defined.",
                 child.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    buffer_mode = fuchsia_hardware_gpio::BufferMode::kOutputLow;
  }

  if (child.GetProperty<bool>("output-high")) {
    if (buffer_mode) {
      fdf::error("Gpio init hog '{}' has more than one buffer mode property defined.",
                 child.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    buffer_mode = fuchsia_hardware_gpio::BufferMode::kOutputHigh;
  }

  if (!buffer_mode) {
    fdf::error("Gpio init hog '{}' does not have a buffer_mode", child.name());

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto gpio_cell_size = parent.GetProperty<uint32_t>("#gpio-cells");
  if (gpio_cell_size.is_error()) {
    fdf::error("Gpio controller '{}' does not have '#gpio-cells' property: {}", parent.name(),
               gpio_cell_size);

    return gpio_cell_size.take_error();
  }

  auto gpios_bytes = gpios->second.AsBytes();
  size_t entry_size = (*gpio_cell_size) * sizeof(uint32_t);

  if (gpios_bytes.size_bytes() % *gpio_cell_size != 0) {
    fdf::error(
        "Gpio init hog '{}' has incorrect number of gpio cells ({}) - expected multiple of {} cells.",
        child.name(), gpios_bytes.size_bytes() / sizeof(uint32_t), *gpio_cell_size);

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  for (size_t byte_idx = 0; byte_idx < gpios_bytes.size_bytes(); byte_idx += entry_size) {
    auto gpio = GpioCells(gpios->second.AsBytes().subspan(byte_idx, entry_size));
    zx::result flags = gpio.flags();
    if (flags.is_error()) {
      fdf::error("Failed to get input flags for gpio init hog '{}' with gpio pin {} : {}",
                 child.name(), gpio.pin(), flags);

      return flags.take_error();
    }

    controller.metadata.init_steps()->push_back(fuchsia_hardware_pinimpl::InitStep::WithCall({{
        .pin = gpio.pin(),
        .call = InitCall::WithPinConfig({{.pull = *gpio.flags()}}),
    }}));
    controller.metadata.init_steps()->push_back(fuchsia_hardware_pinimpl::InitStep::WithCall({{
        .pin = gpio.pin(),
        .call = InitCall::WithBufferMode(*buffer_mode),
    }}));

    fdf::debug("Gpio init step (pin {:#x}) added to controller '{}'", gpio.pin(), parent.name());
  }

  return zx::ok();
}

GpioImplVisitor::GpioController& GpioImplVisitor::GetController(uint32_t node_id) {
  if (!gpio_controllers_.contains(node_id)) {
    gpio_controllers_[node_id] = GpioController();
  }
  return gpio_controllers_[node_id];
}

zx::result<> GpioImplVisitor::ParseReferenceChild(fdf_devicetree::Node& child,
                                                  fdf_devicetree::ReferenceNode& parent,
                                                  fdf_devicetree::PropertyCells specifiers,
                                                  std::optional<std::string_view> gpio_name) {
  if (!gpio_name) {
    fdf::error("Gpio reference '{}' does not have a name", child.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  auto reference_name = std::string(*gpio_name);
  auto& controller = GetController(parent.id());

  if (specifiers.size_bytes() != 2 * sizeof(uint32_t)) {
    fdf::error("Gpio reference '{}' has incorrect number of gpio specifiers ({}) - expected 2.",
               child.name(), specifiers.size_bytes() / sizeof(uint32_t));

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto cells = GpioCells(specifiers);
  auto it = std::find_if(
      controller.metadata.pins()->begin(), controller.metadata.pins()->end(),
      [&cells](const fuchsia_hardware_pinimpl::Pin& entry) { return entry.pin() == cells.pin(); });
  uint32_t global_id;
  if (it == controller.metadata.pins()->end()) {
    global_id = pin_id_counter_++;
    fuchsia_hardware_pinimpl::Pin pin{{
        .pin = cells.pin(),
        .name = reference_name,
        .id = global_id,
    }};
    fdf::debug("Gpio pin added - pin {:#x} id {} name '{}' to controller '{}'", cells.pin(),
               global_id, reference_name, parent.name());
    controller.metadata.pins()->push_back(std::move(pin));
  } else {
    global_id = it->id().value();
  }

  return AddChildNodeSpec(child, global_id, reference_name);
}

zx::result<> GpioImplVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  // Check that it is indeed a gpio-controller that we support.
  if (!is_match(node.properties())) {
    return zx::ok();
  }

  auto controller = gpio_controllers_.find(node.id());
  if (controller == gpio_controllers_.end()) {
    fdf::info("Gpio controller '{}' is not being used. Not adding any metadata for it.",
              node.name());

    return zx::ok();
  }

  {
    fuchsia_hardware_pinimpl::Metadata metadata = {{.controller_id = controller->first}};
    if (!controller->second.metadata.init_steps()->empty()) {
      metadata.init_steps() = *std::move(controller->second.metadata.init_steps());
    }
    if (!controller->second.metadata.pins()->empty()) {
      metadata.pins() = *std::move(controller->second.metadata.pins());
    }
    if (controller->second.metadata.device_pin_states() &&
        !controller->second.metadata.device_pin_states()->empty()) {
      metadata.device_pin_states() = *std::move(controller->second.metadata.device_pin_states());
    }

    const fit::result persisted_pin_metadata = fidl::Persist(metadata);
    if (!persisted_pin_metadata.is_ok()) {
      fdf::error("Failed to encode pin metadata for node {}: {}", node.name(),
                 persisted_pin_metadata.error_value().FormatDescription());

      return zx::error(persisted_pin_metadata.error_value().status());
    }

    fuchsia_hardware_platform_bus::Metadata pin_metadata = {{
        .id = fuchsia_hardware_pinimpl::Metadata::kSerializableName,
        .data = std::move(persisted_pin_metadata.value()),
    }};
    node.AddMetadata(std::move(pin_metadata));

    fdf::debug("Gpio metadata added to node '{}'", node.name());
  }

  return zx::ok();
}

}  // namespace gpio_impl_dt
