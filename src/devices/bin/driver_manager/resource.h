// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_RESOURCE_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_RESOURCE_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/function.h>

#include <memory>
#include <optional>
#include <string>
#include <vector>

#include "src/devices/bin/driver_manager/node_types.h"

namespace driver_manager {

class Node;

// Represents a resource provided by a Node.
class Resource : public fidl::WireServer<fuchsia_driver_framework::ResourceController>,
                 public std::enable_shared_from_this<Resource> {
 public:
  Resource(ResourceId id, std::weak_ptr<Node> owner, std::string name,
           std::vector<fuchsia_driver_framework::NodeProperty2> properties,
           std::vector<NodeOffer> node_offers,
           std::optional<fuchsia_driver_framework::BusInfo> bus_info,
           async_dispatcher_t* dispatcher);
  ~Resource() override = default;

  void set_is_self_resource(bool is_self_resource) { is_self_resource_ = is_self_resource; }

  void Bind(fidl::ServerEnd<fuchsia_driver_framework::ResourceController> server_end);

  // Removes this resource by notifying all dependent nodes that this resource
  // is gone and unregistering this resource from its owning node if it's a provided node.
  void Remove();

  void AddDependent(std::weak_ptr<Node> dependent) { dependents_.push_back(std::move(dependent)); }
  void RemoveDependent(const std::shared_ptr<Node>& dependent);

  std::weak_ptr<Node> owner() const { return owner_; }
  const std::optional<fuchsia_driver_framework::BusInfo>& bus_info() const { return bus_info_; }
  const std::vector<NodeOffer>& offers() const { return offers_; }
  const std::vector<fuchsia_driver_framework::NodeProperty2>& properties() const {
    return properties_;
  }
  ResourceId id() const { return id_; }
  bool is_self_resource() const { return is_self_resource_; }
  const std::vector<std::weak_ptr<Node>>& dependents() const { return dependents_; }

  // Exposed for testing.
  const std::string& name() const { return name_; }

 private:
  // fidl::WireServer<fuchsia_driver_framework::ResourceController>
  void Remove(RemoveCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_framework::ResourceController> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  ResourceId id_;
  std::string name_;
  std::vector<fuchsia_driver_framework::NodeProperty2> properties_;
  std::vector<NodeOffer> offers_;
  std::optional<fuchsia_driver_framework::BusInfo> bus_info_;

  // The Nodes that depend on this resource.
  std::vector<std::weak_ptr<Node>> dependents_;

  // The Node that owns and provides this resource.
  std::weak_ptr<Node> owner_;

  async_dispatcher_t* dispatcher_;
  std::optional<fidl::ServerBinding<fuchsia_driver_framework::ResourceController>> binding_;
  bool is_self_resource_ = false;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_RESOURCE_H_
