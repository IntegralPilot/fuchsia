// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_NODE_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_NODE_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/devicetree/devicetree.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <cstdint>
#include <optional>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <vector>

#include "lib/driver/devicetree/manager/publisher.h"

namespace fdf_devicetree {

using Phandle = uint32_t;
using NodeID = uint32_t;

class Visitor;
class ReferenceNode;
class ParentNode;
class ChildNode;

// Helper to select return type for GetProperty.
template <typename T>
struct GetPropertyReturn {
  using type = zx::result<T>;
};

template <>
struct GetPropertyReturn<bool> {
  using type = bool;
};

// Represents who provides the `reg` property for this node. This information will be set and used
// by the visitors. By default `reg` property of all nodes are considered mmio.
enum class RegisterType : uint8_t {
  kMmio,  // Default. Parsed by the mmio visitor.
  kI2c,   // Register used to represent i2c device address.
  kSpi,   // Register used to represent spi device address.
  kSpmi,  // Register used to represent spmi target id and device registers (sub target id).
  kPci,   // Register used to represent a PCI bus/device/function (BDF) address.
};

// Defines interface that an entity managing the Node should implement.
class NodeManager {
 public:
  // Returns node with phandle |id|.
  virtual zx::result<ReferenceNode> GetReferenceNode(Phandle id) = 0;

  virtual uint32_t GetPublishIndex(uint32_t node_id) = 0;

  virtual zx::result<> ChangePublishOrder(uint32_t node_id, uint32_t new_index) = 0;

  // Registers an iommu with the platform bus.
  virtual zx::result<> RegisterIommu(uint32_t iommu_id,
                                     fuchsia_hardware_platform_bus::Iommu iommu) = 0;

  // Returns true if the node at |path| is enabled by override.
  virtual bool IsNodeForceEnabled(std::string_view path) = 0;

  virtual ~NodeManager();
};

// Node represents the nodes in the device tree along with it's properties.
class Node {
 public:
  explicit Node(Node* parent, std::string_view name, devicetree::Properties properties, uint32_t id,
                NodeManager* manager);
  virtual ~Node() = default;

  // Add |prop| as a bind property of the device, when it is eventually published.
  virtual void AddBindProperty(const fuchsia_driver_framework::NodeProperty2& prop);

  virtual void AddMmio(const fuchsia_hardware_platform_bus::Mmio& mmio);

  virtual void AddBti(const fuchsia_hardware_platform_bus::Bti& bti);

  virtual void AddIrq(const fuchsia_hardware_platform_bus::Irq& irq);

  virtual void AddMetadata(const fuchsia_hardware_platform_bus::Metadata& metadata,
                           std::optional<std::string> fidl_text = std::nullopt);

  virtual void AddBootMetadata(const fuchsia_hardware_platform_bus::BootMetadata& boot_metadata);

  virtual void AddNodeSpec(const fuchsia_driver_framework::ParentSpec2& spec);

  virtual void AddSmc(const fuchsia_hardware_platform_bus::Smc& smc);

  virtual void AddPowerConfig(const fuchsia_hardware_power::PowerElementConfiguration& config,
                              std::optional<std::string> fidl_text = std::nullopt);

  // Registers an iommu with the platform bus.
  virtual zx::result<> RegisterIommu(uint32_t iommu_id,
                                     const fuchsia_hardware_platform_bus::Iommu& iommu) {
    return manager_->RegisterIommu(iommu_id, iommu);
  }

  // Sets the driver host that the driver that binds to this node will end up in.
  void SetDriverHost(std::string_view driver_host) { driver_host_ = driver_host; }

  // Returns the index of the node in the nodes publish list.
  uint32_t GetPublishIndex() const;

  // Move this node up/down in the publish list.
  // Returns error if the index is out of range.
  zx::result<> ChangePublishOrder(uint32_t new_index);

  // Publish this node.
  zx::result<> Publish(PublisherInterface& publisher);

  const std::string& name() const { return name_; }
  const std::string& fdf_name() const { return fdf_name_; }
  std::string path() const;

  std::string_view driver_host() const { return driver_host_; }

  ParentNode parent() const;

  std::vector<ChildNode> children();

  const std::unordered_map<std::string_view, devicetree::PropertyValue>& properties() const {
    return properties_;
  }

  template <typename T>
  typename GetPropertyReturn<T>::type GetProperty(std::string_view property_name) const;

  zx::result<ReferenceNode> GetReferenceNode(Phandle parent);

  std::optional<Phandle> phandle() const { return phandle_; }

  NodeID id() const { return id_; }

  RegisterType register_type() const { return register_type_; }

  void set_register_type(RegisterType type) { register_type_ = type; }

  void set_interrupt_controller_id(uint32_t id);

 private:
  Node* parent_;
  std::string name_;
  std::string fdf_name_;
  std::string driver_host_;
  std::unordered_map<std::string_view, devicetree::PropertyValue> properties_;
  std::optional<Phandle> phandle_;
  std::vector<Node*> children_;

  // Platform bus node.
  fuchsia_hardware_platform_bus::Node pbus_node_;

  // Stores text representation of metadata for golden file generation.
  std::vector<std::optional<std::string>> metadata_text_;

  // Stores text representation of power config for golden file generation.
  std::vector<std::optional<std::string>> power_config_text_;

  // Properties of the nodes after they have been transformed in the device group.
  std::vector<fuchsia_driver_framework::NodeProperty2> node_properties_;

  // Parent specifications.
  std::vector<fuchsia_driver_framework::ParentSpec2> parents_;

  // This is a unique ID we use to match our device group with the correct
  // platform bus node. It is generated at runtime and not stable across boots.
  NodeID id_;

  // Boolean to indicate if a platform device needs to added.
  bool add_platform_device_ = false;

  // Storing handle to manager. This is ok as the manager always outlives the node instance.
  NodeManager* manager_;

  RegisterType register_type_ = RegisterType::kMmio;
};

class ReferenceNode {
 public:
  explicit ReferenceNode(Node* node) : node_(node) {}

  const std::unordered_map<std::string_view, devicetree::PropertyValue>& properties() const {
    return node_->properties();
  }

  template <typename T>
  typename GetPropertyReturn<T>::type GetProperty(std::string_view property_name) {
    return node_->GetProperty<T>(property_name);
  }

  const std::string& name() const { return node_->name(); }
  const std::string& fdf_name() const { return node_->fdf_name(); }

  uint32_t id() const { return node_->id(); }

  std::optional<Phandle> phandle() const { return node_->phandle(); }

  Node* GetNode() const { return node_; }

  ParentNode parent() const;

  explicit operator bool() const { return (node_ != nullptr); }

 private:
  Node* node_;
};

class ParentNode {
 public:
  explicit ParentNode(Node* node) : node_(node) {}

  const std::string& name() const { return node_->name(); }
  const std::string& fdf_name() const { return node_->fdf_name(); }

  uint32_t id() const { return node_->id(); }

  explicit operator bool() const { return (node_ != nullptr); }

  const std::unordered_map<std::string_view, devicetree::PropertyValue>& properties() const {
    return node_->properties();
  }

  template <typename T>
  typename GetPropertyReturn<T>::type GetProperty(std::string_view property_name) {
    return node_->GetProperty<T>(property_name);
  }

  Node* GetNode() const { return node_; }

  ParentNode parent() const { return node_->parent(); }

  ReferenceNode MakeReferenceNode() const { return ReferenceNode(node_); }

 private:
  Node* node_;
};

class ChildNode {
 public:
  explicit ChildNode(Node* node) : node_(node) {}

  const std::string& name() const { return node_->name(); }
  const std::string& fdf_name() const { return node_->fdf_name(); }

  uint32_t id() const { return node_->id(); }

  explicit operator bool() const { return (node_ != nullptr); }

  const std::unordered_map<std::string_view, devicetree::PropertyValue>& properties() const {
    return node_->properties();
  }

  template <typename T>
  typename GetPropertyReturn<T>::type GetProperty(std::string_view property_name) const {
    return node_->GetProperty<T>(property_name);
  }

  Node* GetNode() const { return node_; }

  void AddNodeSpec(const fuchsia_driver_framework::ParentSpec2& spec) { node_->AddNodeSpec(spec); }

  void set_register_type(RegisterType type) { node_->set_register_type(type); }

  RegisterType register_type() const { return node_->register_type(); }

 private:
  Node* node_;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_NODE_H_
