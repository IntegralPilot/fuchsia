// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_POWER_ALL_DRIVERS_ELEMENT_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_POWER_ALL_DRIVERS_ELEMENT_H_

#include <fidl/fuchsia.power.broker/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/defer.h>

#include <memory>

#include "src/devices/bin/driver_manager/node.h"

namespace driver_manager {

class PowerManager;

// AllDriversElement is an ElementRunner server that handles leasing all leaf driver nodes in
// the system.
class AllDriversElement : public fidl::WireServer<fuchsia_power_broker::ElementRunner>,
                          public std::enable_shared_from_this<AllDriversElement> {
 public:
  friend class AllDriversElementTest;

  AllDriversElement(PowerManager* power_manager, std::shared_ptr<Node> root);

  void SetLevel(SetLevelRequestView request, SetLevelCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void AttachElement(async_dispatcher_t* dispatcher, PowerElementHandles handles);

  // Called by DriverRunner whenever a new node is bound. If the node is a leaf driver instance,
  // it will be added to |leaf_driver_instances_|.
  void OnNodeBound(std::shared_ptr<const Node> node);

 private:
  // Transfers startup leases from leaf nodes over to this element. Should only be called once
  // when the system first switches to the "on" level, marking the end of the boot phase.
  //
  // It traverses the topology bottom-up (post-order) to identify leaf driver nodes and
  // extracts the startup lease from every bound node:
  //   - Leaves: Are added to |leaf_driver_instances_|. Their startup lease is kept in |leases_|
  //             or a new lease is asynchronously requested.
  //   - Non-leaves: Their startup lease is temporarily placed into a list so it is
  //                 kept alive until all leaf asynchronous acquisitions finish.
  void CompleteStartupTransition();

  // Processes startup transition logic for a single |node|:
  // If the node is not bound, does nothing.
  // Otherwise, retrieves the node's startup lease.
  //   - If |has_bound_descendant| is false (the node is a leaf driver instance), adds it to
  //     |leaf_driver_instances_|. If it has a valid startup lease, transfers it to |leases_|.
  //   - If |has_bound_descendant| is true, the node is not a leaf; its startup lease is dropped.
  void ProcessStartupNode(const std::shared_ptr<const Node>& node, bool has_bound_descendant);

  // Asynchronously acquires a lease for the given node. |deferred| will be executed when the
  // operation completes (and all references to its shared pointer are destroyed).
  void AcquireLease(std::shared_ptr<const Node> node,
                    std::shared_ptr<fit::deferred_callback> deferred);
  void RemoveAncestors(const std::shared_ptr<const Node>& node);

  fidl::ServerBindingGroup<fuchsia_power_broker::ElementRunner> element_runner_bindings_;
  fidl::Client<fuchsia_power_broker::ElementControl> element_control_;
  fidl::Client<fuchsia_power_broker::Lessor> lessor_;

  // Contains all leaf driver instances in the node topology. Maps the node's weak pointer to
  // its topological path.
  std::unordered_map<std::string, std::weak_ptr<const Node>> leaf_driver_instances_;

  // Leases are acquired when the level is set to 1 for all current leaf nodes,
  // and when a new leaf node is bound. They are dropped when the level is set to 0,
  // or when a node is no longer a driver-bounded leaf node.
  // The keys in |leases_| must match with |leaf_driver_instances_|.
  std::unordered_map<std::string, zx::eventpair> leases_;

  // The current power level of this element. 0 represents low power, 1 represents the active/on
  // state.
  uint32_t current_level_ = 0;

  // Set to true after the initial startup transition has finished.
  // During boot, this is false. The startup transition is completed when the element's power
  // level is first set to 1, at which point all startup leases are transferred from the leaf nodes
  // to this element.
  bool transition_complete_ = false;

  // A weak pointer to the root of the node topology, used to discover all nodes during the
  // startup transition.
  std::weak_ptr<Node> root_;

  // Owns this AllDriversElement.
  PowerManager* power_manager_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_POWER_ALL_DRIVERS_ELEMENT_H_
