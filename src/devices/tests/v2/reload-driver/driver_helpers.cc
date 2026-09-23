// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/tests/v2/reload-driver/driver_helpers.h"

#include <fidl/fuchsia.reloaddriver.test/cpp/fidl.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/reloaddriverbind/test/cpp/bind.h>

namespace bindlib = bind_fuchsia_reloaddriverbind_test;
namespace ft = fuchsia_reloaddriver_test;

namespace reload_test_driver_helpers {
zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fdf::Logger& logger, std::string_view child_name,
    fidl::SyncClient<fuchsia_driver_framework::Node>& node_client,
    std::string_view child_test_property) {
  fuchsia_driver_framework::NodeProperty2 node_property =
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, child_test_property);
  fuchsia_driver_framework::NodeAddArgs args(
      {.name = std::string(child_name), .properties2 = {{node_property}}});

  // Create endpoints of the `NodeController` for the node.
  auto endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }

  auto result = node_client->AddChild({std::move(args), std::move(endpoints->server), {}});
  if (result.is_error()) {
    FDF_LOGL(ERROR, logger, "AddChild failed: %s",
             result.error_value().FormatDescription().c_str());
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(endpoints->client));
}

zx::result<> SendAck(fdf::Logger& logger, std::string_view node_name,
                     const std::shared_ptr<fdf::Namespace>& incoming,
                     std::string_view driver_name) {
  auto connect_result = incoming->Connect<ft::Waiter>();
  if (connect_result.is_error()) {
    FDF_LOGL(ERROR, logger, "Failed to connect to ft::Waiter: %s", connect_result.status_string());
    return connect_result.take_error();
  }

  fidl::SyncClient<ft::Waiter> waiter(std::move(connect_result.value()));
  auto result = waiter->Ack({std::string(node_name), std::string(driver_name), ZX_OK});
  if (result.is_error()) {
    FDF_LOGL(ERROR, logger, "Ack failed: %s", result.error_value().FormatDescription().c_str());
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

}  // namespace reload_test_driver_helpers
