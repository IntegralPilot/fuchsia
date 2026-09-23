// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_GPIO_CONTROLLERS_GPIOIMPL_VISITOR_GPIOIMPL_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_GPIO_CONTROLLERS_GPIOIMPL_VISITOR_GPIOIMPL_VISITOR_H_

#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pinimpl/cpp/fidl.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <cstdint>
#include <memory>

namespace gpio_impl_dt {

class GpioImplVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kGpioReference[] = "gpios";
  static constexpr char kGpioCells[] = "#gpio-cells";
  static constexpr char kGpioNames[] = "gpio-names";

  static constexpr char kPinCtrl0[] = "pinctrl-0";
  static constexpr char kPins[] = "pins";
  static constexpr char kPinFunction[] = "function";
  static constexpr char kPinFunctionId[] = "function-id";
  static constexpr char kPinDriveStrengthUa[] = "drive-strength-microamp";
  static constexpr char kPinDriveStrength[] = "drive-strength";
  static constexpr char kPinOutputLow[] = "output-low";
  static constexpr char kPinOutputHigh[] = "output-high";
  static constexpr char kPinOutputDisable[] = "output-disable";
  static constexpr char kPinBiasPullDown[] = "bias-pull-down";
  static constexpr char kPinBiasPullUp[] = "bias-pull-up";
  static constexpr char kPinBiasDisable[] = "bias-disable";
  static constexpr char kPinDrivePushPull[] = "drive-push-pull";
  static constexpr char kPinDriveOpenDrain[] = "drive-open-drain";
  static constexpr char kPinDriveOpenSource[] = "drive-open-source";
  static constexpr char kPinPowerSource[] = "power-source";
  static constexpr char kPinWakeVector[] = "fuchsia,interrupt-wake-vector";

  GpioImplVisitor();

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  struct GpioController {
    GpioController() {
      metadata.init_steps().emplace();
      metadata.pins().emplace();
    }

    fuchsia_hardware_pinimpl::Metadata metadata;
  };

  // Return an existing or a new instance of GpioController.
  GpioController& GetController(uint32_t node_id);

  // Helper to parse nodes with a reference to gpio-controller in "gpios" property.
  zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                   fdf_devicetree::ReferenceNode& parent,
                                   fdf_devicetree::PropertyCells specifiers,
                                   std::optional<std::string_view> gpio_name);

  // Helper to parse gpio init hog to produce fuchsia_hardware_pinimpl::InitStep.
  zx::result<> ParseGpioHogChild(fdf_devicetree::Node& child);

  // Helper to parse pin controller configuration as it relates to gpio.
  zx::result<> ParsePinCtrlCfg(fdf_devicetree::Node& child, fdf_devicetree::ReferenceNode& cfg_node,
                               fdf_devicetree::ParentNode& gpio_node);

  // Helper to parse pin controller state config.
  zx::result<> ParsePinCtrlStateCfg(
      fdf_devicetree::ReferenceNode& cfg_node, fdf_devicetree::ParentNode& gpio_node,
      std::vector<fuchsia_hardware_pinimpl::PinConfiguration>& pin_configs);

  zx::result<> ParsePinStates(fdf_devicetree::Node& node);
  zx::result<> ParseBootTimeConfig(fdf_devicetree::Node& node);

  static zx::result<fuchsia_hardware_pin::Configuration> ParsePinConfiguration(
      fdf_devicetree::ReferenceNode& cfg_node);
  static zx::result<std::optional<fuchsia_hardware_gpio::BufferMode>> ParseBufferMode(
      fdf_devicetree::ReferenceNode& cfg_node);

  static zx::result<fdf_devicetree::ParentNode> GetGpioNodeForPinConfig(
      fdf_devicetree::ReferenceNode& cfg_node);

  static zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t global_id,
                                       const std::string& gpio_name);

  static zx::result<> AddInitNodeSpec(fdf_devicetree::Node& child, uint32_t controller_id,
                                      uint32_t controller_index);

  static zx::result<> AddPinStatesNodeSpec(fdf_devicetree::Node& child, uint32_t controller_id,
                                           uint32_t controller_index,
                                           const std::string& client_name);

  static std::string GetUniqueNodeName(fdf_devicetree::Node& node);

  static bool is_match(
      const std::unordered_map<std::string_view, devicetree::PropertyValue>& properties) {
    return properties.contains("gpio-controller");
  }

  // Mapping of gpio controller node ID to its info.
  std::map<uint32_t, GpioController> gpio_controllers_;
  std::unique_ptr<fdf_devicetree::PropertyParser> gpio_parser_;
  uint32_t pin_id_counter_ = 0;
};

}  // namespace gpio_impl_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_GPIO_CONTROLLERS_GPIOIMPL_VISITOR_GPIOIMPL_VISITOR_H_
