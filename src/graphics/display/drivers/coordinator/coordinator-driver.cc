// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/coordinator-driver.h"

#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/coordinator/controller.h"
#include "src/graphics/display/drivers/coordinator/structured_config.h"

namespace display_coordinator {

CoordinatorDriver::CoordinatorDriver()
    : fdf::DriverBase2("display-coordinator"),
      devfs_connector_(fit::bind_member<&CoordinatorDriver::ConnectProvider>(this)) {}

CoordinatorDriver::~CoordinatorDriver() = default;

zx::result<> CoordinatorDriver::Start(fdf::DriverContext context) {
  auto config = context.take_config<structured_config::Config>();

  inspect::Inspector inspector;
  inspect::ComponentInspector component_inspector = context.CreateInspector(this, inspector);

  std::shared_ptr<fdf::Namespace> incoming_ptr(context.take_incoming());
  auto create_engine_driver_client_result = EngineDriverClient::Create(incoming_ptr);
  if (create_engine_driver_client_result.is_error()) {
    fdf::error("Failed to create EngineDriverClient: {}", create_engine_driver_client_result);
    return create_engine_driver_client_result.take_error();
  }

  zx::result<std::unique_ptr<Controller>> create_controller_result =
      Controller::Create(std::move(create_engine_driver_client_result).value(),
                         driver_dispatcher()->borrow(), std::move(inspector),
                         config.fallback_horizontal_size_mm(), config.fallback_vertical_size_mm());
  if (create_controller_result.is_error()) {
    fdf::error("Failed to create Controller: {}", create_controller_result);
    return create_controller_result.take_error();
  }

  controller_ = std::move(create_controller_result).value();

  // Create a node for devfs.
  zx::result<fidl::ClientEnd<fuchsia_device_fs::Connector>> bind_devfs_connector_result =
      devfs_connector_.Bind(dispatcher());
  if (bind_devfs_connector_result.is_error()) {
    fdf::error("Failed to bind to devfs connector: {}", bind_devfs_connector_result);
    return bind_devfs_connector_result.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs{{
      .connector = std::move(bind_devfs_connector_result).value(),
      .class_name = "display-coordinator",
      .connector_supports = fuchsia_device_fs::ConnectionType::kDevice,
  }};

  zx::result<fdf::OwnedChildNode> add_child_result = AddOwnedChild(name(), devfs);
  if (add_child_result.is_error()) {
    fdf::error("Failed to add child node: {}", add_child_result);
    return add_child_result.take_error();
  }

  auto [controller_client, node_client] = std::move(add_child_result).value();
  node_controller_.Bind(std::move(controller_client));
  node_.Bind(std::move(node_client));
  return zx::ok();
}

void CoordinatorDriver::Stop(fdf::StopCompleter completer) {
  controller_->PrepareStop();
  completer(zx::ok());
}

void CoordinatorDriver::ConnectProvider(
    fidl::ServerEnd<fuchsia_hardware_display::Provider> provider_request) {
  provider_bindings_.AddBinding(dispatcher(), std::move(provider_request), controller_.get(),
                                fidl::kIgnoreBindingClosure);
}

}  // namespace display_coordinator

FUCHSIA_DRIVER_EXPORT2(display_coordinator::CoordinatorDriver);
