// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.rebind.test/cpp/fidl.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/legacy-bind-constants/legacy-bind-constants.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <zircon/errors.h>

#include <optional>
#include <string>
#include <unordered_map>

#include <bind/fuchsia/cpp/bind.h>

#include "lib/zx/result.h"

namespace rebind_parent {

const std::string kDriverName = "rebind-parent";
const std::string kChildNodeName = "added-child";

class RebindParentServer : public fidl::Server<fuchsia_rebind_test::RebindParent> {
 public:
  RebindParentServer(async_dispatcher_t* dispatcher,
                     fidl::ClientEnd<fuchsia_driver_framework::Node> node)
      : node_(std::move(node)) {}

 private:
  void RemoveChild(RemoveChildCompleter::Sync& completer) override {
    if (!node_controller_.has_value()) {
      fdf::error("Child device does not exist");
      completer.Reply(fit::error(ZX_ERR_BAD_STATE));
      return;
    }

    auto status = node_controller_.value()->Remove();
    if (status.status() != ZX_OK) {
      FDF_SLOG(ERROR, "Failed to remove node", KV("status", status.status_string()));
      completer.Reply(fit::error(status.status()));
      return;
    }
    completer.Reply(fit::ok());
  }

  void AddChild(AddChildCompleter::Sync& completer) override {
    fidl::Arena arena;

    auto properties = fidl::VectorView<fuchsia_driver_framework::wire::NodeProperty2>(arena, 1);
    properties[0] = fdf::MakeProperty2(arena, bind_fuchsia::PROTOCOL, 1234u);

    auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                    .name(arena, kChildNodeName)
                    .properties2(properties)
                    .Build();

    // Create endpoints of the `NodeController` for the node.
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
    if (endpoints.is_error()) {
      FDF_SLOG(ERROR, "Failed to create endpoint", KV("status", endpoints.status_string()));
      completer.Reply(fit::error(endpoints.status_value()));
      return;
    }
    auto result = node_->AddChild(args, std::move(endpoints->server), {});
    if (!result.ok()) {
      FDF_SLOG(ERROR, "Failed to add child", KV("status", result.status_string()));
      completer.Reply(fit::error(result.status()));
      return;
    }
    node_controller_.emplace(std::move(endpoints->client));
    completer.Reply(fit::ok());
  }

  fidl::WireSyncClient<fuchsia_driver_framework::Node> node_;
  std::optional<fidl::WireSyncClient<fuchsia_driver_framework::NodeController>> node_controller_;
};

class RebindParent : public fdf::DriverBase2 {
 public:
  RebindParent()
      : DriverBase2(kDriverName), devfs_connector_(fit::bind_member<&RebindParent::Serve>(this)) {}

  zx::result<> Start(fdf::DriverContext context) override {
    // Export ourselves to devfs.
    fidl::Arena arena;
    zx::result connector = devfs_connector_.Bind(dispatcher());
    if (connector.is_error()) {
      return connector.take_error();
    }

    auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena).connector(
        std::move(connector.value()));

    auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                    .name(arena, name())
                    .devfs_args(devfs.Build())
                    .Build();

    // Create endpoints of the `NodeController` for the node.
    auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

    zx::result node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
    ZX_ASSERT_MSG(node_endpoints.is_ok(), "Failed: %s", node_endpoints.status_string());

    fidl::WireResult result = fidl::WireCall(node())->AddChild(
        args, std::move(controller_endpoints.server), std::move(node_endpoints->server));
    if (!result.ok()) {
      FDF_SLOG(ERROR, "Failed to add child", KV("status", result.status_string()));
      return zx::error(result.status());
    }
    controller_.Bind(std::move(controller_endpoints.client));
    server_.emplace(dispatcher(), std::move(node_endpoints->client));

    return zx::ok();
  }

 private:
  void Serve(fidl::ServerEnd<fuchsia_rebind_test::RebindParent> server) {
    fidl::BindServer(dispatcher(), std::move(server), &server_.value());
  }

  std::optional<RebindParentServer> server_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  driver_devfs::Connector<fuchsia_rebind_test::RebindParent> devfs_connector_;
};

}  // namespace rebind_parent

FUCHSIA_DRIVER_EXPORT2(rebind_parent::RebindParent);
