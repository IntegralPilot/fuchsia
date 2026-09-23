// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "i2c-bus-visitor.h"

#include <fidl/fuchsia.hardware.i2c.businfo/cpp/fidl.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <algorithm>
#include <cstdint>
#include <optional>
#include <utility>

#include <bind/fuchsia/cpp/bind.h>
#include <fbl/string_printf.h>

namespace i2c_bus_dt {

bool I2cBusVisitor::is_match(fdf_devicetree::Node& node) {
  if (node.name().find("i2c@") == std::string::npos) {
    return false;
  }

  auto address_cells = node.GetProperty<uint32_t>("#address-cells");
  if (address_cells.is_error() || *address_cells != 1) {
    return false;
  }

  auto size_cells = node.GetProperty<uint32_t>("#size-cells");
  if (size_cells.is_error() || *size_cells != 0) {
    return false;
  }

  return true;
}

void I2cBusVisitor::AddChildNodeSpec(fdf_devicetree::ChildNode& child, uint32_t global_id,
                                     std::string_view name) {
  auto i2c_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, global_id),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, name),
          },
  }};
  child.AddNodeSpec(i2c_node);
}

zx::result<> I2cBusVisitor::CreateController(std::string node_name) {
  if (i2c_controllers_.contains(node_name)) {
    fdf::error("Failed to create I2C Controller. An I2C controller with name '{}' already exists.",
               node_name);

    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }
  i2c_controllers_[node_name] = I2cController();
  i2c_controllers_[node_name].bus_id = bus_id_counter_++;
  return zx::ok();
}

zx::result<> I2cBusVisitor::ParseChild(I2cController& controller, fdf_devicetree::Node& parent,
                                       fdf_devicetree::ChildNode& child) {
  // Parse reg to get the address.
  auto reg = child.GetProperty<std::vector<uint32_t>>("reg");
  if (reg.is_error()) {
    fdf::error("I2C child '{}' has no reg property: {}.", child.name(), reg);

    return reg.take_error();
  }

  if (reg->empty()) {
    fdf::error("I2C child '{}' has an empty reg property.", child.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto reg_names = child.GetProperty<std::vector<std::string>>("reg-names");

  for (size_t i = 0; i < reg->size(); ++i) {
    const uint32_t address = (*reg)[i];
    const auto it = std::ranges::find_if(controller.channels, [address](const auto& ch) {
      return ch.address() && *ch.address() == address;
    });

    uint32_t global_id;
    if (it == controller.channels.end()) {
      global_id = channel_id_counter_++;
      fuchsia_hardware_i2c_businfo::I2CChannel channel;
      channel.address() = address;
      channel.global_id() = global_id;

      std::string child_name;
      if (reg->size() > 1) {
        child_name = fbl::StringPrintf("%s-0x%02x", child.name().c_str(), address).c_str();
      } else {
        child_name = child.name();
      }
      channel.name() = std::move(child_name);

      controller.channels.emplace_back(std::move(channel));
      fdf::debug("I2c channel '{}' added at address {:#x} to controller '{}'",
                 *controller.channels.back().name(), address, parent.name());
    } else {
      global_id = *it->global_id();
      fdf::debug(
          "I2c channel at address {:#x} already exists in controller '{}', skipping metadata addition",
          address, parent.name());
    }

    std::string name = "i2c";
    if (reg_names.is_ok() && i < reg_names->size()) {
      if ((*reg_names)[i].starts_with("i2c-")) {
        name = (*reg_names)[i];
      } else {
        name = "i2c-" + (*reg_names)[i];
      }
    }
    AddChildNodeSpec(child, global_id, name);
  }

  return zx::ok();
}

zx::result<> I2cBusVisitor::Visit(fdf_devicetree::Node& node,
                                  const devicetree::PropertyDecoder& decoder) {
  if (is_match(node)) {
    auto result = CreateController(node.name());
    if (result.is_error()) {
      return result.take_error();
    }
  }
  return zx::ok();
}

zx::result<> I2cBusVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  // Check that it is indeed a i2c-controller that we support.
  if (!is_match(node)) {
    return zx::ok();
  }

  auto controller = i2c_controllers_.find(node.name());
  ZX_ASSERT_MSG(controller != i2c_controllers_.end(), "i2c controller '%s' entry not found.",
                node.name().c_str());

  for (auto& child : node.children()) {
    auto result = ParseChild(controller->second, node, child);
    if (result.is_error()) {
      fdf::error("Failed to parse i2c child '{}' : {}", child.name(), result);

      return result.take_error();
    }
  }

  if (!controller->second.channels.empty()) {
    fuchsia_hardware_i2c_businfo::I2CBusMetadata bus_metadata = {{
        .channels = controller->second.channels,
        .bus_id = controller->second.bus_id,
    }};
    auto encoded_bus_metadata = fidl::Persist(bus_metadata);
    if (encoded_bus_metadata.is_error()) {
      fdf::info("Failed to persist fidl metadata for i2c controller '{}': {}", node.name(),
                encoded_bus_metadata.error_value().FormatDescription());

      return zx::ok();
    }
    node.AddMetadata({{
        .id = fuchsia_hardware_i2c_businfo::I2CBusMetadata::kSerializableName,
        .data = std::move(encoded_bus_metadata.value()),
    }});
    fdf::debug("I2C channels metadata added to node '{}'", node.name());
  }

  return zx::ok();
}

}  // namespace i2c_bus_dt

REGISTER_DEVICETREE_VISITOR(i2c_bus_dt::I2cBusVisitor);
