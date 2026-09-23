// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_REGULATOR_REGULATOR_REGULATOR_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_REGULATOR_REGULATOR_REGULATOR_VISITOR_H_

#include <lib/driver/devicetree/manager/node.h>
#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <optional>
#include <string_view>

namespace regulator_visitor_dt {

class RegulatorVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kRegulatorReference[] = "regulators";
  static constexpr char kRegulatorFunctions[] = "regulator-functions";
  static constexpr char kRegulatorCells[] = "#regulator-cells";
  static constexpr char kRegulatorName[] = "regulator-name";
  static constexpr char kRegulatorMinMicrovolt[] = "regulator-min-microvolt";
  static constexpr char kRegulatorMaxMicrovolt[] = "regulator-max-microvolt";
  static constexpr char kRegulatorStepMicrovolt[] = "regulator-step-microvolt";

  RegulatorVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  bool is_match(const std::string& name);
  zx::result<> AddRegulatorMetadata(fdf_devicetree::Node& node,
                                    fdf_devicetree::ParsedProperties& values);
  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, fdf_devicetree::ReferenceNode& parent,
                                std::optional<std::string_view> function);

  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
  std::unique_ptr<fdf_devicetree::PropertyParser> reference_parser_;
};

}  // namespace regulator_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_REGULATOR_REGULATOR_REGULATOR_VISITOR_H_
