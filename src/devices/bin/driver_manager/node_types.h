// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_TYPES_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_TYPES_H_

#include <fidl/fuchsia.component.decl/cpp/wire.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>

#include <cstdint>
#include <format>
#include <string>
#include <string_view>
#include <vector>

namespace driver_manager {

using ResourceId = uint32_t;

enum class Collection : uint8_t {
  kNone,
  // Collection for boot drivers.
  kBoot,
  // Collection for package drivers.
  kPackage,
  // Collection for universe package drivers.
  kFullPackage,
};

enum class OfferTransport : std::uint8_t {
  DriverTransport,
  ZirconTransport,
  Dictionary,
};

struct NodeOffer {
  std::string source_name;
  Collection source_collection;
  OfferTransport transport;
  std::string service_name;
  std::vector<std::string> source_instance_filter;
  std::vector<fuchsia_component_decl::NameMapping> renamed_instances;
};

fuchsia_driver_framework::Offer ToFidl(const NodeOffer& offer);

fit::result<fuchsia_driver_framework::NodeError, NodeOffer> ProcessNodeOffer(
    const fuchsia_driver_framework::Offer& add_offer, Collection source_collection,
    std::string_view source_name);

// This function creates a composite offer based on a service offer.
NodeOffer CreateCompositeOffer(const NodeOffer& offer, std::string_view parents_name,
                               bool primary_parent);

enum class NodeType {
  kNormal,     // Normal non-composite node.
  kComposite,  // Composite node created from composite node specs.
};

enum class DriverState : uint8_t {
  kStopped,  // Driver is not running.
  kBinding,  // Driver's bind hook is scheduled and running.
  kRunning,  // Driver finished binding and is running.
};

enum class NodeState : uint8_t {
  kRunning,              // Normal running state.
  kPrestop,              // Still running, but will remove soon. usually because the node
                         // received Remove(kPackage), but is a boot driver.
  kWaitingOnDriverBind,  // Waiting for the driver to complete binding.
  kWaitingOnChildren,    // Received Remove, and waiting for children to be removed.

  kWaitingOnDriver,           // Waiting for driver to respond from Stop() command.
  kWaitingOnDriverComponent,  // Waiting for driver component to be stopped.
  kStopped,                   // Node finished stopping.
  kWaitingOnDestroy,          // Waiting for the component to be destroyed.
  kDestroyed,                 // Node finished destroying component.
};

}  // namespace driver_manager

template <>
struct std::formatter<driver_manager::OfferTransport> : std::formatter<std::string_view> {
  auto format(const driver_manager::OfferTransport& transport, std::format_context& ctx) const {
    switch (transport) {
      // Dictionary maps to ZirconTransport.
      case driver_manager::OfferTransport::ZirconTransport:
      case driver_manager::OfferTransport::Dictionary:
        return std::formatter<std::string_view>::format("ZirconTransport", ctx);
      case driver_manager::OfferTransport::DriverTransport:
        return std::formatter<std::string_view>::format("DriverTransport", ctx);
    }
  }
};

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_TYPES_H_
