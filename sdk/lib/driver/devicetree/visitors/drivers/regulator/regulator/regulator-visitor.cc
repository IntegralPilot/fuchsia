// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "regulator-visitor.h"

#include <fidl/fuchsia.hardware.vreg/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/errors.h>

#include <memory>
#include <regex>
#include <string>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>

namespace regulator_visitor_dt {

RegulatorVisitor::RegulatorVisitor() {
  fdf_devicetree::Properties reference_properties = {};
  reference_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kRegulatorReference, kRegulatorCells, /* required */ false));
  reference_properties.emplace_back(std::make_unique<fdf_devicetree::StringListProperty>(
      kRegulatorFunctions, /* required */ false));
  reference_parser_ =
      std::make_unique<fdf_devicetree::PropertyParser>(std::move(reference_properties));

  fdf_devicetree::Properties regulator_properties = {};
  regulator_properties.emplace_back(
      std::make_unique<fdf_devicetree::StringProperty>(kRegulatorName));
  regulator_properties.emplace_back(std::make_unique<fdf_devicetree::Uint32Property>(
      kRegulatorMaxMicrovolt, /* required */ false));
  regulator_properties.emplace_back(std::make_unique<fdf_devicetree::Uint32Property>(
      kRegulatorMinMicrovolt, /* required */ false));
  regulator_properties.emplace_back(std::make_unique<fdf_devicetree::Uint32Property>(
      kRegulatorStepMicrovolt, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(regulator_properties));
}

bool RegulatorVisitor::is_match(const std::string& name) {
  std::regex name_regex(".*regulator(@.*)?$");
  return std::regex_match(name, name_regex);
}

zx::result<> RegulatorVisitor::Visit(fdf_devicetree::Node& node,
                                     const devicetree::PropertyDecoder& decoder) {
  if (is_match(node.name())) {
    auto parser_output = parser_->Parse(node);
    if (parser_output.is_error()) {
      fdf::error("Regulator visitor failed for node '{}' : {}", node.name(), parser_output);

      return parser_output.take_error();
    }

    auto status = AddRegulatorMetadata(node, *parser_output);
    if (status.is_error()) {
      fdf::error("Failed to add regulator metadata '{}' : {}", node.name(), status);

      return status.take_error();
    }
  }

  auto reference_output = reference_parser_->Parse(node);
  if (reference_output.is_error()) {
    fdf::error("Regulator visitor failed for node '{}' : {}", node.name(), reference_output);

    return reference_output.take_error();
  }

  auto references = reference_output->Get<fdf_devicetree::References>(kRegulatorReference);
  if (!references) {
    return zx::ok();
  }

  std::optional<std::vector<std::string>> functions =
      reference_output->Get<std::vector<std::string>>(kRegulatorFunctions);
  if (functions.has_value() && functions.value().size() != references.value().size()) {
    fdf::error(
        "Regulator reference '{}' has mismatched number of regulator-functions"
        " ({}) vs regulators ({}).",
        node.name(), functions.value().size(), references.value().size());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  for (size_t index = 0; index < references.value().size(); ++index) {
    auto& reference = references.value()[index];
    auto reference_node = reference.reference_node();
    std::optional<std::string_view> function;
    if (functions.has_value()) {
      function = functions.value()[index];
    }

    auto status = AddChildNodeSpec(node, reference_node, function);
    if (status.is_error()) {
      fdf::error("Failed to add regulator '{}' node spec to '{}' : {}",
                 reference.reference_node().name(), node.name(), status);

      return status.take_error();
    }
  }

  return zx::ok();
}

zx::result<> RegulatorVisitor::AddRegulatorMetadata(fdf_devicetree::Node& node,
                                                    fdf_devicetree::ParsedProperties& properties) {
  auto name = properties.Get<std::string>(kRegulatorName);
  if (!name) {
    fdf::error("Regulator node '{}' does not have a name.", node.name());

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  fuchsia_hardware_vreg::VregMetadata metadata;
  metadata.name() = *name;

  auto min_uv = properties.Get<uint32_t>(kRegulatorMinMicrovolt);
  if (min_uv) {
    metadata.min_voltage_uv() = *min_uv;
  }

  auto step_uv = properties.Get<uint32_t>(kRegulatorStepMicrovolt);
  if (step_uv) {
    metadata.voltage_step_uv() = *step_uv;
  }

  auto max_uv = properties.Get<uint32_t>(kRegulatorMaxMicrovolt);
  if (max_uv && metadata.voltage_step_uv() && metadata.min_voltage_uv()) {
    if (*max_uv < *metadata.min_voltage_uv()) {
      fdf::error("Regulator max voltage ({}) is not more than min voltage ({}) in node '{}'",
                 *max_uv, *metadata.min_voltage_uv(), node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    auto voltage_range = (*max_uv - *metadata.min_voltage_uv());

    if (voltage_range % (*metadata.voltage_step_uv()) != 0) {
      fdf::error("Voltage range ({}) is not a multiple of step size ({}) for node '{}'",
                 voltage_range, *metadata.voltage_step_uv(), node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    metadata.num_steps() = (voltage_range / *metadata.voltage_step_uv()) + 1;
  }

  fit::result persisted_metadata = fidl::Persist(metadata);
  if (!persisted_metadata.is_ok()) {
    fdf::error("Failed to persist vreg metadata for node {}: {}", node.name(),
               persisted_metadata.error_value().FormatDescription());

    return zx::error(persisted_metadata.error_value().status());
  }
  fuchsia_hardware_platform_bus::Metadata vreg_metadata = {{
      .id = fuchsia_hardware_vreg::VregMetadata::kSerializableName,
      .data = std::move(persisted_metadata.value()),
  }};

  node.AddMetadata(std::move(vreg_metadata));
  fdf::debug("Added regulator metadata to node '{}'.", node.name());

  return zx::ok();
}

zx::result<> RegulatorVisitor::AddChildNodeSpec(fdf_devicetree::Node& child,
                                                fdf_devicetree::ReferenceNode& parent,
                                                std::optional<std::string_view> function) {
  auto regulator_name = parent.GetProperty<std::string>(kRegulatorName);
  if (regulator_name.is_error()) {
    fdf::error("Regulator node '{}' does not have a name: {}.", parent.name(), regulator_name);

    return regulator_name.take_error();
  }

  std::string property_name =
      function.has_value() ? std::string(function.value()) : *regulator_name;

  auto regulator_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.vreg.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::NAME, *regulator_name),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.vreg.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, property_name),
          },
  }};

  child.AddNodeSpec(regulator_node);
  fdf::debug("Added regulator node spec of '{}' to '{}'.", parent.name(), child.name());

  return zx::ok();
}

}  // namespace regulator_visitor_dt

REGISTER_DEVICETREE_VISITOR(regulator_visitor_dt::RegulatorVisitor);
