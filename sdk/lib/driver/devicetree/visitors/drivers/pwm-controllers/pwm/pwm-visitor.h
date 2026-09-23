// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PWM_CONTROLLERS_PWM_PWM_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PWM_CONTROLLERS_PWM_PWM_VISITOR_H_

#include <fidl/fuchsia.hardware.pwm/cpp/fidl.h>
#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace pwm_visitor_dt {

enum class PwmFlags : uint32_t {
  PWM_POLARITY_INVERTED = 0x1,
  PWM_SKIP_INIT = 0x2,
};

class PwmVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kPwmReference[] = "pwms";
  static constexpr char kPwmCells[] = "#pwm-cells";
  static constexpr char kPwmNames[] = "pwm-names";

  PwmVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

 private:
  struct PwmController {
    fuchsia_hardware_pwm::PwmChannelsMetadata pwm_channels;
  };

  // Return an existing or a new instance of PwmController.
  PwmController& GetController(fdf_devicetree::Phandle phandle);

  bool is_match(const std::string& name);

  zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                   fdf_devicetree::ReferenceNode& parent,
                                   fdf_devicetree::PropertyCells specifiers,
                                   std::optional<std::string_view> pwm_name);

  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t id,
                                std::optional<std::string_view> pwm_name);

  // Mapping of pwm controller node phandle to its info.
  std::map<fdf_devicetree::Phandle, PwmController> pwm_controllers_;
  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace pwm_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PWM_CONTROLLERS_PWM_PWM_VISITOR_H_
