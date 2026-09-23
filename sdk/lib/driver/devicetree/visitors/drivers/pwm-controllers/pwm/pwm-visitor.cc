// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pwm-visitor.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <cstdint>
#include <regex>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>

namespace {
using fuchsia_hardware_pwm::PwmChannelInfo;
}

namespace pwm_visitor_dt {

PwmVisitor::PwmVisitor() {
  fdf_devicetree::Properties pwm_properties = {};
  pwm_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kPwmReference, kPwmCells, /* required */ false));
  pwm_properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kPwmNames, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(pwm_properties));
}

bool PwmVisitor::is_match(const std::string& name) {
  std::regex name_regex("^pwm@[0-9a-f]+$");
  return std::regex_match(name, name_regex);
}

zx::result<> PwmVisitor::Visit(fdf_devicetree::Node& node,
                               const devicetree::PropertyDecoder& decoder) {
  zx::result parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("PWM visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  auto pwms = parser_output->Get<fdf_devicetree::References>(kPwmReference);
  if (!pwms) {
    return zx::ok();
  }

  auto pwm_names = parser_output->Get<std::vector<std::string>>(kPwmNames);
  if (!pwm_names && pwms->size() != 1u) {
    fdf::error(
        "PWM reference '{}' does not have valid pwm names property. Name is required to generate bind rules, especially when more than one pwm is referenced.",
        node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (pwm_names && pwm_names->size() != pwms->size()) {
    fdf::error(
        "PWM reference '{}' does not expected number of pwm names property. Expected: {} actual: {}.",
        node.name(), pwms->size(), pwm_names->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  for (uint32_t index = 0; index < pwms->size(); index++) {
    auto& reference = (*pwms)[index];
    if (is_match(reference.reference_node().name())) {
      std::optional<std::string_view> name;
      if (pwm_names) {
        name = (*pwm_names)[index];
      }
      auto result =
          ParseReferenceChild(node, reference.reference_node(), reference.property_cells(), name);
      if (result.is_error()) {
        return result.take_error();
      }
    }
  }

  return zx::ok();
}

PwmVisitor::PwmController& PwmVisitor::GetController(fdf_devicetree::Phandle phandle) {
  const auto [controller_iter, success] = pwm_controllers_.insert({phandle, PwmController()});
  return controller_iter->second;
}

zx::result<> PwmVisitor::ParseReferenceChild(fdf_devicetree::Node& child,
                                             fdf_devicetree::ReferenceNode& parent,
                                             fdf_devicetree::PropertyCells specifiers,
                                             std::optional<std::string_view> pwm_name) {
  auto& controller = GetController(*parent.phandle());

  if (specifiers.size_bytes() < 1 * sizeof(uint32_t)) {
    fdf::error(
        "PWM reference '{}' has incorrect number of pwm specifiers ({}) - expected at least 1.",
        child.name(), specifiers.size_bytes() / sizeof(uint32_t));

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto cells = fdf_devicetree::Uint32Array(specifiers);
  PwmChannelInfo pwm_channel = {};
  pwm_channel.id() = cells[0];

  // Second cell if present specifies the PWM period in nanoseconds.
  if (cells.size() >= 2) {
    pwm_channel.period_ns() = cells[1];
  }

  // Third cell if present would contain the PWM flags.
  if (cells.size() == 3) {
    if (cells[2] & static_cast<uint32_t>(PwmFlags::PWM_POLARITY_INVERTED)) {
      pwm_channel.polarity() = true;
    }
    if (cells[2] & static_cast<uint32_t>(PwmFlags::PWM_SKIP_INIT)) {
      pwm_channel.skip_init() = true;
    }
  }

  fdf::debug(
      "PWM channel added - ID {:#x}, Period {}, Polarity {}, Skip init {}, and name '{}' to controller '{}'",
      *pwm_channel.id(), pwm_channel.period_ns().value_or(0), pwm_channel.polarity().value_or(0),
      pwm_channel.skip_init().value_or(0), pwm_name.value_or("<anonymous>"), parent.name());

  if (!controller.pwm_channels.channels()) {
    controller.pwm_channels.channels() = std::vector<PwmChannelInfo>();
  }
  controller.pwm_channels.channels()->emplace_back(pwm_channel);

  return AddChildNodeSpec(child, cells[0], pwm_name);
}

zx::result<> PwmVisitor::AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t id,
                                          std::optional<std::string_view> pwm_name) {
  auto pwm_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, id),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
          },
  }};

  if (pwm_name) {
    pwm_node.properties().push_back(fdf::MakeProperty2(bind_fuchsia::NAME, std::string(*pwm_name)));
  }

  child.AddNodeSpec(pwm_node);
  return zx::ok();
}

zx::result<> PwmVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  // Check that it is indeed a pwm that we support.
  if (!is_match(node.name())) {
    return zx::ok();
  }

  if (node.phandle()) {
    auto controller = pwm_controllers_.find(*node.phandle());
    if (controller == pwm_controllers_.end()) {
      fdf::info("PWM controller '{}' is not being used. Not adding any metadata for it.",
                node.name());

      return zx::ok();
    }

    if (controller->second.pwm_channels.channels()) {
      fit::result persisted_metadata = fidl::Persist(controller->second.pwm_channels);
      if (persisted_metadata.is_error()) {
        fdf::error("Failed to persist pwm channels metadata: {}",
                   persisted_metadata.error_value().FormatDescription());

        return zx::error(persisted_metadata.error_value().status());
      }

      fuchsia_hardware_platform_bus::Metadata channels_metadata = {{
          .id = fuchsia_hardware_pwm::PwmChannelsMetadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }};

      node.AddMetadata(std::move(channels_metadata));
      fdf::debug("PWM Channels metadata added to node '{}'", node.name());
    }
  }
  return zx::ok();
}

}  // namespace pwm_visitor_dt

REGISTER_DEVICETREE_VISITOR(pwm_visitor_dt::PwmVisitor);
