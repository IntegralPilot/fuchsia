// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_NODE_CPP_ADD_CHILD_H_
#define LIB_DRIVER_NODE_CPP_ADD_CHILD_H_

#include <fidl/fuchsia.driver.framework/cpp/markers.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/stdcompat/span.h>

// The following functions can be used to add various types of child nodes to a parent node.
// There are two main types of nodes, owned and un-owned.
//
// Owned: An owned node belongs to the driver creating it, so the driver framework will not try to
// match it up to another driver. Therefore when you create an owned node, you must maintain the
// connection to the node by holding on to the client end.
//
// Un-owned: These are nodes that are not owned by the creating driver. The driver framework will
// go through the driver matching process to find a matching driver, and start the matched driver
// and bind it to the node. The node can provide resources to the bound driver through its offers.
//
// Once the type of node being created is chosen, the next step is to choose whether the node
// should export itself into devfs. The driver framework team is currently in the process of
// deprecating devfs in favor of service capabilities, but until that solution is available, devfs
// is how non-driver components can connect to driver provided protocols. If your node needs to
// provide devfs support, that is available through the variants that support the DevfsAddArgs.
namespace fdf {

// Contains the client ends for an owned child node that was created using the helper functions.
// This must be handled since the driver must ensure not to drop the node client.
struct [[nodiscard]] OwnedChildNode {
  // This is a client to the node controller. This is safe to drop if not needed.
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> node_controller_;

  // The client MUST keep this, as dropping it will result in the added node to be removed by the
  // driver framework.
  fidl::ClientEnd<fuchsia_driver_framework::Node> node_;
};

// Adds an owned child node under the given |parent|. The driver framework will NOT try to match
// and bind a driver to this child as it is already owned by the current driver.
//
// This is a synchronous call and requires that the dispatcher allow sync calls.
zx::result<OwnedChildNode> AddOwnedChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name);

// Adds an un-owned child node under the given |parent|. The driver framework will try to match
// and bind a driver to this child.
//
// This is a synchronous call and requires that the dispatcher allow sync calls.
zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name,
    cpp20::span<const fuchsia_driver_framework::NodeProperty> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers);

// Creates an owned child node with devfs support under the given |parent|. The driver framework
// will NOT try to match and bind a driver to this child as it is already owned by the current
// driver.
//
// This is a synchronous call and requires that the dispatcher allow sync calls.
zx::result<OwnedChildNode> AddOwnedChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args);

// Creates an un-owned child node with devfs support under the given |parent|. The driver framework
// will try to match and bind a driver to this child.
//
// This is a synchronous call and requires that the dispatcher allow sync calls.
zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args,
    cpp20::span<const fuchsia_driver_framework::NodeProperty> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers);

// Adds an un-owned child node under the given |parent|. The driver framework will try to match
// and bind a driver to this child.
//
// This is a synchronous call and requires that the dispatcher allow sync calls.
zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name,
    cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers);

// Adds an un-owned child node under the given |parent|. The driver framework will try to match
// and bind a driver to this child.
//
// This is a synchronous call and requires that the dispatcher allow sync calls.
zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args,
    cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers);

}  // namespace fdf

#endif  // LIB_DRIVER_NODE_CPP_ADD_CHILD_H_
