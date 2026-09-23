// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "adc-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <algorithm>
#include <cstdint>
#include <memory>
#include <optional>
#include <string_view>

#include <bind/fuchsia/adc/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>

namespace adc_dt {

namespace {

class AdcCells {
 public:
  explicit AdcCells(fdf_devicetree::PropertyCells cells) : adc_cells_(cells, 1) {}

  // 1st cell denotes the adc channel.
  uint32_t channel() { return static_cast<uint32_t>(*adc_cells_[0][0]); }

 private:
  using AdcElement = devicetree::PropEncodedArrayElement<1>;
  devicetree::PropEncodedArray<AdcElement> adc_cells_;
};

}  // namespace

void AdcVisitor::AdcController::AddChannel(uint32_t chan_idx, const std::string& name,
                                           const std::string& parent_name) {
  fuchsia_hardware_adcimpl::AdcChannel channel;
  channel.idx() = chan_idx;
  channel.name() = name;
  fdf::debug("Adc channel added - channel {:#x} name '{}' to controller '{}'", *channel.idx(),
             *channel.name(), parent_name);

  // Insert if the channel is not already present.
  auto it = std::find_if(channels.begin(), channels.end(),
                         [&channel](const fuchsia_hardware_adcimpl::AdcChannel& entry) {
                           return entry.idx() == *channel.idx();
                         });
  if (it == channels.end()) {
    channels.emplace_back(std::move(channel));
  }
}

bool AdcVisitor::is_match(fdf_devicetree::Node& node) {
  return (node.name().find("adc@") != std::string::npos) && IioVisitor::is_match(node);
}

AdcVisitor::AdcController& AdcVisitor::GetController(uint32_t node_id) {
  if (!controllers_.contains(node_id)) {
    controllers_[node_id] = AdcController();
  }
  return controllers_[node_id];
}

zx::result<> AdcVisitor::AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t chan_id) {
  auto adc_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia_adc::CHANNEL, chan_id),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
              fdf::MakeProperty2(bind_fuchsia_adc::CHANNEL, chan_id),
          },
  }};
  child.AddNodeSpec(adc_node);
  return zx::ok();
}

zx::result<> AdcVisitor::ParseReferenceChild(fdf_devicetree::Node& child,
                                             fdf_devicetree::ReferenceNode& parent,
                                             fdf_devicetree::PropertyCells specifiers,
                                             std::optional<std::string_view> name) {
  if (!name) {
    fdf::error("Adc reference '{}' does not have a name", child.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  auto reference_name = std::string(*name);

  if (specifiers.size_bytes() != 1 * sizeof(uint32_t)) {
    fdf::error("Adc reference '{}' has incorrect number of adc specifiers ({}) - expected 1.",
               child.name(), specifiers.size_bytes() / sizeof(uint32_t));

    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const auto chan_idx = AdcCells(specifiers).channel();

  GetController(parent.id()).AddChannel(chan_idx, reference_name, parent.name());

  return AddChildNodeSpec(child, chan_idx);
}

zx::result<> AdcVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  // Check that it is indeed a adc-controller that we support.
  if (!is_match(node)) {
    return zx::ok();
  }

  auto controller = controllers_.find(node.id());
  if (controller == controllers_.end()) {
    fdf::info("ADC controller '{}' is not being used. Not adding any metadata for it.",
              node.name());

    return zx::ok();
  }

  {
    fuchsia_hardware_adcimpl::Metadata metadata;
    metadata.channels() = std::move(controller->second.channels);

    const fit::result encoded_controller_metadata = fidl::Persist(metadata);
    if (!encoded_controller_metadata.is_ok()) {
      fdf::error("Failed to encode ADC controller metadata for node {}: {}", node.name(),
                 encoded_controller_metadata.error_value().FormatDescription());

      return zx::error(encoded_controller_metadata.error_value().status());
    }
    fuchsia_hardware_platform_bus::Metadata controller_metadata = {{
        .id = fuchsia_hardware_adcimpl::Metadata::kSerializableName,
        .data = encoded_controller_metadata.value(),
    }};
    node.AddMetadata(std::move(controller_metadata));
    fdf::debug("ADC controller metadata added to node '{}'", node.name());
  }

  return zx::ok();
}

}  // namespace adc_dt

REGISTER_DEVICETREE_VISITOR(adc_dt::AdcVisitor);
