// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.framework/cpp/natural_messaging.h>
#include <lib/driver/node/cpp/add_child.h>

namespace fdf {

zx::result<OwnedChildNode> AddOwnedChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name) {
  auto [node_controller_client_end, node_controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  auto [node_client_end, node_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  fuchsia_driver_framework::NodeAddArgs args{{
      .name = {std::string(node_name)},
  }};

  fidl::Result<fuchsia_driver_framework::Node::AddChild> result = fidl::Call(parent)->AddChild(
      {std::move(args), std::move(node_controller_server_end), std::move(node_server_end)});

  if (result.is_error()) {
    logger.log(fdf::LogSeverity::ERROR, "Failed to add owned child {}. Error: {}", node_name,
               result.error_value().FormatDescription());
    return zx::error(result.error_value().is_framework_error()
                         ? result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok(OwnedChildNode{std::move(node_controller_client_end), std::move(node_client_end)});
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name,
    cpp20::span<const fuchsia_driver_framework::NodeProperty> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  auto [node_controller_client_end, node_controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  std::vector<fuchsia_driver_framework::NodeProperty> props{properties.begin(), properties.end()};
  std::vector<fuchsia_driver_framework::Offer> offers2{offers.begin(), offers.end()};

  fuchsia_driver_framework::NodeAddArgs args{{
      .name = {std::string(node_name)},
      .properties = std::move(props),
      .offers2 = std::move(offers2),
  }};

  fidl::Result<fuchsia_driver_framework::Node::AddChild> result =
      fidl::Call(parent)->AddChild({std::move(args), std::move(node_controller_server_end), {}});

  if (result.is_error()) {
    logger.log(fdf::LogSeverity::ERROR, "Failed to add child {}. Error: {}", node_name,
               result.error_value().FormatDescription());
    return zx::error(result.error_value().is_framework_error()
                         ? result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(node_controller_client_end));
}

zx::result<OwnedChildNode> AddOwnedChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args) {
  auto [node_controller_client_end, node_controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  auto [node_client_end, node_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  fuchsia_driver_framework::NodeAddArgs args{{
      .name = {std::string(node_name)},
      .devfs_args = std::move(devfs_args),
  }};

  fidl::Result<fuchsia_driver_framework::Node::AddChild> result = fidl::Call(parent)->AddChild(
      {std::move(args), std::move(node_controller_server_end), std::move(node_server_end)});

  if (result.is_error()) {
    logger.log(fdf::LogSeverity::ERROR, "Failed to add owned devfs child {}. Error: {}", node_name,
               result.error_value().FormatDescription());
    return zx::error(result.error_value().is_framework_error()
                         ? result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok(OwnedChildNode{std::move(node_controller_client_end), std::move(node_client_end)});
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args,
    cpp20::span<const fuchsia_driver_framework::NodeProperty> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  auto [node_controller_client_end, node_controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  std::vector<fuchsia_driver_framework::NodeProperty> props{properties.begin(), properties.end()};
  std::vector<fuchsia_driver_framework::Offer> offers2{offers.begin(), offers.end()};

  fuchsia_driver_framework::NodeAddArgs args{{
      .name = {std::string(node_name)},
      .properties = std::move(props),
      .devfs_args = std::move(devfs_args),
      .offers2 = std::move(offers2),
  }};

  fidl::Result<fuchsia_driver_framework::Node::AddChild> result =
      fidl::Call(parent)->AddChild({std::move(args), std::move(node_controller_server_end), {}});

  if (result.is_error()) {
    logger.log(fdf::LogSeverity::ERROR, "Failed to add devfs child {}. Error: {}", node_name,
               result.error_value().FormatDescription());
    return zx::error(result.error_value().is_framework_error()
                         ? result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(node_controller_client_end));
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name,
    cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  auto [node_controller_client_end, node_controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  std::vector<fuchsia_driver_framework::NodeProperty2> props{properties.begin(), properties.end()};
  std::vector<fuchsia_driver_framework::Offer> offers2{offers.begin(), offers.end()};

  fuchsia_driver_framework::NodeAddArgs args{{
      .name = {std::string(node_name)},
      .offers2 = std::move(offers2),
      .properties2 = std::move(props),
  }};

  fidl::Result<fuchsia_driver_framework::Node::AddChild> result =
      fidl::Call(parent)->AddChild({std::move(args), std::move(node_controller_server_end), {}});

  if (result.is_error()) {
    logger.log(fdf::LogSeverity::ERROR, "Failed to add child {}. Error: {}", node_name,
               result.error_value().FormatDescription());
    return zx::error(result.error_value().is_framework_error()
                         ? result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(node_controller_client_end));
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args,
    cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  auto [node_controller_client_end, node_controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  std::vector<fuchsia_driver_framework::NodeProperty2> props{properties.begin(), properties.end()};
  std::vector<fuchsia_driver_framework::Offer> offers2{offers.begin(), offers.end()};

  fuchsia_driver_framework::NodeAddArgs args{{
      .name = {std::string(node_name)},
      .devfs_args = std::move(devfs_args),
      .offers2 = std::move(offers2),
      .properties2 = std::move(props),
  }};

  fidl::Result<fuchsia_driver_framework::Node::AddChild> result =
      fidl::Call(parent)->AddChild({std::move(args), std::move(node_controller_server_end), {}});

  if (result.is_error()) {
    logger.log(fdf::LogSeverity::ERROR, "Failed to add devfs child {}. Error: {}", node_name,
               result.error_value().FormatDescription());
    return zx::error(result.error_value().is_framework_error()
                         ? result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(node_controller_client_end));
}

}  // namespace fdf
