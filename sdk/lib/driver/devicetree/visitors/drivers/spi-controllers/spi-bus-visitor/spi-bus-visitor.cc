// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/devicetree/visitors/drivers/spi-controllers/spi-bus-visitor/spi-bus-visitor.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/errors.h>

#include <algorithm>

#include <bind/fuchsia/cpp/bind.h>

namespace spi_bus_dt {

zx::result<> SpiBusVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  if (!is_match(node)) {
    return zx::ok();
  }

  auto controller = spi_controllers_.find(node.name());
  ZX_ASSERT_MSG(controller != spi_controllers_.end(), "SPI controller '%s' entry not found",
                node.name().c_str());

  for (auto& child : node.children()) {
    if (zx::result<> result = ParseChild(controller->second, node, child); result.is_error()) {
      return result.take_error();
    }
  }

  if (controller->second.channels.empty()) {
    return zx::ok();
  }

  const fuchsia_hardware_spi_businfo::SpiBusMetadata bus_metadata = {{
      .channels = controller->second.channels,
      .bus_id = controller->second.bus_id,
  }};
  fit::result encoded_bus_metadata = fidl::Persist(bus_metadata);
  if (encoded_bus_metadata.is_error()) {
    fdf::info("Failed to persist FIDL metadata for SPI controller '{}': {}", node.name(),
              encoded_bus_metadata.error_value().FormatDescription());

    return zx::error(encoded_bus_metadata.error_value().status());
  }
  node.AddMetadata({{
      .id = fuchsia_hardware_spi_businfo::SpiBusMetadata::kSerializableName,
      .data = std::move(encoded_bus_metadata.value()),
  }});
  fdf::debug("SPI channels metadata added to node '{}'", node.name());

  return zx::ok();
}

zx::result<> SpiBusVisitor::Visit(fdf_devicetree::Node& node,
                                  const devicetree::PropertyDecoder& decoder) {
  if (is_match(node)) {
    if (zx::result<> result = CreateController(node.name()); result.is_error()) {
      return result.take_error();
    }
  }
  return zx::ok();
}

zx::result<> SpiBusVisitor::CreateController(const std::string& node_name) {
  if (spi_controllers_.contains(node_name)) {
    fdf::error("Duplicate SPI controller '{}'", node_name);

    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  spi_controllers_[node_name] = SpiController{.bus_id = bus_id_counter_++};
  return zx::ok();
}

void SpiBusVisitor::AddChildNodeSpec(fdf_devicetree::ChildNode& child, uint32_t global_id) {
  child.AddNodeSpec(fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, global_id),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "spi"),
          },
  }});
}

zx::result<> SpiBusVisitor::ParseChild(SpiController& controller, fdf_devicetree::Node& parent,
                                       fdf_devicetree::ChildNode& child) {
  auto reg = child.GetProperty<std::vector<uint32_t>>("reg");
  if (reg.is_error()) {
    // Ignore config child nodes.
    if (child.name() == "fuchsia,config") {
      return zx::ok();
    }

    fdf::error("SPI child '{}' has no reg property: {}", child.name(), reg);

    return reg.take_error();
  }

  auto max_frequency = child.GetProperty<uint32_t>("spi-max-frequency");

  for (uint32_t i = 0; i < reg->size(); i++) {
    const uint32_t chip_select = (*reg)[i];

    const auto it =
        std::find_if(controller.channels.cbegin(), controller.channels.cend(),
                     [chip_select](const fuchsia_hardware_spi_businfo::SpiChannel& other) {
                       return other.cs() == chip_select;
                     });
    if (it != controller.channels.cend()) {
      fdf::error("Duplicate reg property {} for SPI controller '{}'", chip_select, parent.name());

      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }

    fdf::debug("SPI channel {} to controller '{}'", chip_select, parent.name());

    uint32_t global_id = channel_id_counter_++;
    fuchsia_hardware_spi_businfo::SpiChannel channel{{
        .cs = chip_select,
        .global_id = global_id,
    }};
    if (max_frequency.is_ok()) {
      channel.max_frequency_hz(*max_frequency);
    }
    controller.channels.emplace_back(std::move(channel));
    AddChildNodeSpec(child, global_id);
  }

  return zx::ok();
}

bool SpiBusVisitor::is_match(fdf_devicetree::Node& node) {
  if (node.name().find("spi@") == std::string::npos) {
    return false;
  }

  auto address_cells = node.GetProperty<uint32_t>("#address-cells");
  if (address_cells.is_error() || *address_cells != 1) {
    return false;
  }

  auto size_cells = node.GetProperty<uint32_t>("#size-cells");
  return size_cells.is_ok() && *size_cells == 0;
}

}  // namespace spi_bus_dt

REGISTER_DEVICETREE_VISITOR(spi_bus_dt::SpiBusVisitor);
