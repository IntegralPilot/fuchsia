// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/driver/devicetree/manager/node.h"

#include <endian.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <algorithm>
#include <optional>
#include <string>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace fdf_devicetree {

constexpr const char kPhandleProp[] = "phandle";

NodeManager::~NodeManager() = default;

Node::Node(Node* parent, const std::string_view name, devicetree::Properties properties,
           uint32_t id, NodeManager* manager)
    : parent_(parent), name_(name), id_(id), manager_(manager) {
  ZX_ASSERT(manager_);

  if (parent_) {
    parent_->children_.push_back(this);
  } else {
    name_ = "dt-root";
  }

  fdf_name_ = name_;
  // '@' and ',' are not a valid character in Node names as per driver framework.
  std::ranges::replace(fdf_name_, '@', '-');
  std::ranges::replace(fdf_name_, ',', '-');

  pbus_node_.did() = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_DEVICETREE;
  pbus_node_.vid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC;
  pbus_node_.instance_id() = id;
  pbus_node_.name() = fdf_name_;

  for (auto property : properties) {
    properties_.emplace(property.name, property.value);
  }

  // Get phandle if exists.
  auto phandle = GetProperty<uint32_t>(kPhandleProp);
  if (phandle.is_ok()) {
    phandle_ = phandle.value();
  } else if (phandle.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::warn("Node '{}' has invalid phandle property: {}", name_, phandle.status_value());
  }
}

void Node::AddBindProperty(const fuchsia_driver_framework::NodeProperty2& prop) {
  node_properties_.emplace_back(prop);
}

void Node::AddMmio(const fuchsia_hardware_platform_bus::Mmio& mmio) {
  if (!pbus_node_.mmio()) {
    pbus_node_.mmio() = std::vector<fuchsia_hardware_platform_bus::Mmio>();
  }
  pbus_node_.mmio()->emplace_back(mmio);
  add_platform_device_ = true;
}

void Node::AddBti(const fuchsia_hardware_platform_bus::Bti& bti) {
  if (!pbus_node_.bti()) {
    pbus_node_.bti() = std::vector<fuchsia_hardware_platform_bus::Bti>();
  }
  pbus_node_.bti()->emplace_back(bti);
  add_platform_device_ = true;
}

void Node::AddIrq(const fuchsia_hardware_platform_bus::Irq& irq) {
  if (!pbus_node_.irq()) {
    pbus_node_.irq() = std::vector<fuchsia_hardware_platform_bus::Irq>();
  }
  pbus_node_.irq()->emplace_back(irq);
  add_platform_device_ = true;
}

void Node::AddMetadata(const fuchsia_hardware_platform_bus::Metadata& metadata,
                       std::optional<std::string> fidl_text) {
  if (!pbus_node_.metadata()) {
    pbus_node_.metadata() = std::vector<fuchsia_hardware_platform_bus::Metadata>();
  }
  pbus_node_.metadata()->emplace_back(metadata);
  metadata_text_.emplace_back(std::move(fidl_text));
  add_platform_device_ = true;
}

void Node::AddBootMetadata(const fuchsia_hardware_platform_bus::BootMetadata& boot_metadata) {
  if (!pbus_node_.boot_metadata()) {
    pbus_node_.boot_metadata() = std::vector<fuchsia_hardware_platform_bus::BootMetadata>();
  }
  pbus_node_.boot_metadata()->emplace_back(boot_metadata);
  add_platform_device_ = true;
}

void Node::AddNodeSpec(const fuchsia_driver_framework::ParentSpec2& spec) {
  parents_.emplace_back(spec);
}

void Node::AddSmc(const fuchsia_hardware_platform_bus::Smc& smc) {
  if (!pbus_node_.smc()) {
    pbus_node_.smc() = std::vector<fuchsia_hardware_platform_bus::Smc>();
  }
  pbus_node_.smc()->emplace_back(smc);
  add_platform_device_ = true;
}

void Node::AddPowerConfig(const fuchsia_hardware_power::PowerElementConfiguration& power_config,
                          std::optional<std::string> fidl_text) {
  if (!pbus_node_.power_config()) {
    pbus_node_.power_config() = std::vector<fuchsia_hardware_power::PowerElementConfiguration>();
  }
  pbus_node_.power_config()->emplace_back(power_config);
  power_config_text_.emplace_back(std::move(fidl_text));
  add_platform_device_ = true;
}

uint32_t Node::GetPublishIndex() const { return manager_->GetPublishIndex(id()); }

zx::result<> Node::ChangePublishOrder(uint32_t new_index) {
  return manager_->ChangePublishOrder(id(), new_index);
}

zx::result<> Node::Publish(PublisherInterface& publisher) {
  if (node_properties_.empty() && parents_.empty() && !add_platform_device_) {
    fdf::debug(
        "Not publishing node '{}' because it has no node properties, no platform resources, "
        "and no parent references.",
        name());
    return zx::ok();
  }

  auto status_property = GetProperty<std::string>("status");
  if (status_property.is_ok() && *status_property != "okay") {
    if (!manager_->IsNodeForceEnabled(path())) {
      fdf::debug("Not publishing node '{}' because its status is {}.", name(), *status_property);
      return zx::ok();
    }
    fdf::info("Publishing node '{}' despite status '{}' due to override.", name(),
              *status_property);
  }

  // Nodes are published as per below logic -
  // 1. Node has platform resources -> PlatformBus.NodeAdd + CompositeNodeManager.AddSpec
  // 2. Node does not have platform resources
  //     a. Node has bind properties (i.e. compatible string) ->
  //            Node.AddChild + CompositeNodeManager.AddSpec
  //     b. Node has no bind properties
  //        i. Node references other nodes -> CompositeNodeManager.AddSpec
  //        ii. Node does not reference other nodes -> Not published

  bool add_board_child = !add_platform_device_ && !node_properties_.empty();

  if (add_platform_device_) {
    fdf::debug("Adding node '{}' to pbus with instance id {}.", fdf_name(), id_);

    // Pass properties to pbus node directly if there is no parent node.
    if (parents_.empty()) {
      pbus_node_.properties() = node_properties_;
      if (!driver_host_.empty()) {
        pbus_node_.driver_host() = driver_host_;
      }
    }

    zx::result<> result = publisher.AddPbusNode(pbus_node_, metadata_text_, power_config_text_);
    if (result.is_error()) {
      return result.take_error();
    }
  } else if (add_board_child) {
    fdf::debug("Adding node '{}' as board driver child.", fdf_name());

    fuchsia_driver_framework::BusInfo bus_info{{
        .bus = fuchsia_driver_framework::BusType::kDeviceTree,
        .address = fuchsia_driver_framework::DeviceAddress::WithStringValue(fdf_name()),
        .address_stability = fuchsia_driver_framework::DeviceAddressStability::kStable,
    }};

    auto result = publisher.AddBoardChildNode(
        {.name = fdf_name(),
         .properties = node_properties_,
         .driver_host =
             !driver_host_.empty() ? std::optional<std::string>(driver_host_) : std::nullopt,
         .bus_info = std::move(bus_info)});
    if (result.is_error()) {
      return result.take_error();
    }
  }

  // Add composite node spec.
  if (add_platform_device_) {
    // Construct the platform bus node.
    fdf::ParentSpec2 platform_node;
    platform_node.properties() = node_properties_;
    auto additional_node_properties = std::vector<fdf::NodeProperty2>{
        fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_platform::BIND_PROTOCOL_DEVICE),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                           bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                           bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_DEVICETREE),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, id_),
        fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.platform.device.Service"),
    };
    platform_node.properties().insert(platform_node.properties().end(),
                                      additional_node_properties.begin(),
                                      additional_node_properties.end());

    platform_node.bind_rules() = std::vector<fdf::BindRule2>{
        fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL,
                                bind_fuchsia_platform::BIND_PROTOCOL_DEVICE),
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                                bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC),
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                                bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_DEVICETREE),
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, id_),
    };
    parents_.insert(parents_.begin(), std::move(platform_node));
  } else if (add_board_child) {
    // Construct the non platform bus node.
    fdf::ParentSpec2 board_child_node;
    board_child_node.properties() = node_properties_;

    for (auto& node_property : node_properties_) {
      fdf::BindRule2 bind_rule = {node_property.key(),
                                  fuchsia_driver_framework::Condition::kAccept,
                                  {node_property.value()}};
      board_child_node.bind_rules().emplace_back(std::move(bind_rule));
    }
    parents_.insert(parents_.begin(), std::move(board_child_node));
  }

  fdf::debug("Adding composite node spec to '{}' with {} parents.", fdf_name(), parents_.size());

  zx::result<> result = publisher.AddCompositeNodeSpec(
      fdf_name(), std::move(parents_),
      !driver_host_.empty() ? std::optional<std::string>(driver_host_) : std::nullopt);
  if (result.is_error()) {
    return result.take_error();
  }

  return zx::ok();
}

zx::result<ReferenceNode> Node::GetReferenceNode(Phandle parent) {
  return manager_->GetReferenceNode(parent);
}

ParentNode Node::parent() const { return ParentNode(parent_); }

std::string Node::path() const {
  ParentNode p = parent();
  if (!p) {
    return "/";
  }
  std::string path = p.GetNode()->path();
  if (path != "/") {
    path.append("/");
  }
  path.append(name());
  return path;
}

std::vector<ChildNode> Node::children() {
  std::vector<ChildNode> children;
  children.reserve(children_.size());
  for (Node* child : children_) {
    children.emplace_back(child);
  }
  return children;
}

ParentNode ReferenceNode::parent() const { return node_->parent(); }

template <typename T>
typename GetPropertyReturn<T>::type Node::GetProperty(std::string_view property_name) const {
  auto it = properties_.find(property_name);
  if constexpr (std::is_same_v<T, bool>) {
    return it != properties_.end();
  } else {
    if (it == properties_.end()) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }

    const devicetree::PropertyValue& prop_value = it->second;

    if constexpr (std::is_same_v<T, std::string>) {
      auto val = prop_value.AsString();
      if (val) {
        return zx::ok(std::string(*val));
      }
    } else if constexpr (std::is_same_v<T, uint32_t>) {
      auto val = prop_value.AsUint32();
      if (val) {
        return zx::ok(*val);
      }
    } else if constexpr (std::is_same_v<T, uint64_t>) {
      auto val = prop_value.AsUint64();
      if (val) {
        return zx::ok(*val);
      }
    } else if constexpr (std::is_same_v<T, std::vector<uint32_t>>) {
      auto bytes = prop_value.AsBytes();
      if (bytes.size() % sizeof(uint32_t) != 0) {
        return zx::error(ZX_ERR_WRONG_TYPE);
      }
      std::vector<uint32_t> result;
      result.reserve(bytes.size() / sizeof(uint32_t));
      for (size_t i = 0; i < bytes.size(); i += sizeof(uint32_t)) {
        uint32_t val;
        memcpy(&val, bytes.data() + i, sizeof(uint32_t));
        result.push_back(be32toh(val));
      }
      return zx::ok(result);
    } else if constexpr (std::is_same_v<T, std::vector<std::string>>) {
      auto string_list = prop_value.AsStringList();
      if (string_list) {
        std::vector<std::string> result(string_list->begin(), string_list->end());
        return zx::ok(result);
      }
    } else {
      static_assert(false, "Invalid type for Node::GetProperty");
    }

    return zx::error(ZX_ERR_WRONG_TYPE);
  }
}

template bool Node::GetProperty<bool>(std::string_view property_name) const;
template zx::result<std::string> Node::GetProperty<std::string>(
    std::string_view property_name) const;
template zx::result<uint32_t> Node::GetProperty<uint32_t>(std::string_view property_name) const;
template zx::result<uint64_t> Node::GetProperty<uint64_t>(std::string_view property_name) const;
template zx::result<std::vector<uint32_t>> Node::GetProperty<std::vector<uint32_t>>(
    std::string_view property_name) const;
template zx::result<std::vector<std::string>> Node::GetProperty<std::vector<std::string>>(
    std::string_view property_name) const;

void Node::set_interrupt_controller_id(uint32_t id) {
  pbus_node_.interrupt_controller_id(id);
  add_platform_device_ = true;
}

}  // namespace fdf_devicetree
